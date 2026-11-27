import React, { useCallback, useEffect, useId, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Loader2, Repeat } from 'lucide-react';
import { cn } from '../../lib/utils';
import { InfoPopover } from '../../components/ui/InfoPopover';
import { FIELD_LABEL, FOCUS_RING } from './fields';
import { openInTasks } from './openInTasks';
import {
  ACTION_LABELS,
  HISTORY_SHOWN,
  actionsOf,
  historyRow,
  optionIdOf,
  optionsFromDue,
  parseDue,
  seriesState,
  slotText,
} from './repeat';
import type { RepeatChoice, RepeatRun, RepeatView } from './repeat';
import { REPEAT_CHANGED_EVENT, repeatClear, repeatDecide, repeatGet, repeatRuns, repeatSet } from './repeatApi';
import type { TodoItem } from './types';

const SELECT = cn(
  'h-8 w-56 max-w-full truncate rounded-lg border border-shodh-border bg-shodh-surface px-2 text-small text-shodh-text',
  'hover:border-shodh-border-strong focus:border-shodh-border-strong focus:outline-none focus-visible:ring-2 focus-visible:ring-ring',
  'disabled:opacity-60 disabled:cursor-not-allowed',
);

const ACTION = cn(
  'h-8 px-2.5 inline-flex items-center justify-center rounded-lg border border-shodh-border text-caption font-medium',
  'text-shodh-text hover:bg-shodh-raised transition-colors duration-micro disabled:opacity-60',
  FOCUS_RING,
);

/** Fixed widths, so a spinner in place of a label never moves the row. */
const ACTION_WIDTH: Record<ReturnType<typeof actionsOf>[number], string> = {
  make: 'w-36',
  skip: 'w-28',
  mark_done: 'w-24',
  resume: 'w-24',
  open_latest: 'w-28',
};

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export interface RepeatState {
  view: RepeatView | null;
  runs: RepeatRun[];
  loaded: boolean;
  busy: string | null;
  error: string | null;
  run: (key: string, action: () => Promise<unknown>) => Promise<void>;
}

/** A task's repeat and its history, re-read whenever a task, a repeat or the Inbox changes. */
export function useRepeat(taskId: string | null): RepeatState {
  const [view, setView] = useState<RepeatView | null>(null);
  const [runs, setRuns] = useState<RepeatRun[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const current = useRef(taskId);
  current.current = taskId;

  const reload = useCallback(async () => {
    const id = taskId;
    if (!id) return;
    try {
      const [nextView, nextRuns] = await Promise.all([repeatGet(id), repeatRuns(id)]);
      if (current.current !== id) return;
      setView(nextView);
      setRuns(nextRuns);
      setLoaded(true);
    } catch (err) {
      if (current.current !== id) return;
      setError(errorText(err));
      setLoaded(true);
    }
  }, [taskId]);

  useEffect(() => {
    setView(null);
    setRuns([]);
    setLoaded(false);
    setError(null);
    void reload();
    let active = true;
    const unlisteners: Array<() => void> = [];
    for (const name of [REPEAT_CHANGED_EVENT, 'calendar-changed', 'inbox-changed']) {
      listen(name, () => { void reload(); })
        .then(fn => { if (active) unlisteners.push(fn); else fn(); })
        .catch(err => console.error(`Failed to listen for ${name}:`, err));
    }
    return () => {
      active = false;
      for (const fn of unlisteners) fn();
    };
  }, [reload]);

  const run = useCallback(async (key: string, action: () => Promise<unknown>) => {
    setBusy(key);
    setError(null);
    try {
      await action();
      await reload();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  }, [reload]);

  return { view, runs, loaded, busy, error, run };
}

/**
 * Repeat in the task sheet: a menu, a one-line state and an action row. The state line
 * and the action row are always there (empty ones keep their height), so nothing moves
 * when the state changes. The explanation of each state is behind the (i) button.
 */
export function RepeatField({ task, repeat }: { task: TodoItem; repeat: RepeatState }) {
  const selectId = useId();
  const stateId = useId();
  const { view, busy, error, run } = repeat;
  const due = parseDue(task.dueDate);
  const options = optionsFromDue(task.dueDate);
  const selected = optionIdOf(view?.schedule, options);
  const state = seriesState(view, due !== null);
  const slot = slotText(view, due !== null, due?.allDay ?? false);
  const actions = actionsOf(state, view);

  const choose = (id: string) => {
    if (id === selected || id === 'current') return;
    const option = options.find(o => o.id === id);
    if (!option) return;
    void run('menu', () => (option.schedule ? repeatSet(task.id, option.schedule) : repeatClear(task.id)));
  };

  const act = (action: ReturnType<typeof actionsOf>[number]) => {
    if (!view) return;
    if (action === 'open_latest') {
      openInTasks({ taskId: view.latestTaskId });
      return;
    }
    if (action === 'resume') {
      void run(action, () => repeatSet(task.id, view.schedule));
      return;
    }
    void run(action, () => repeatDecide(view.jobId, action as RepeatChoice));
  };

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-1.5">
        <label htmlFor={selectId} className={cn(FIELD_LABEL, 'inline-flex items-center gap-1.5')}>
          <Repeat className="w-3.5 h-3.5" aria-hidden="true" />
          Repeat
        </label>
        <InfoPopover label="About Repeat" title="Repeat">
          <p>{slot.tooltip}</p>
        </InfoPopover>
      </div>
      <div className="flex items-center gap-2">
        <select
          id={selectId}
          value={selected}
          disabled={state === 'no-due' || busy !== null}
          aria-describedby={stateId}
          title={state === 'no-due' ? 'Set a due date first' : undefined}
          onChange={e => choose(e.target.value)}
          className={cn(SELECT, FOCUS_RING)}
        >
          {selected === 'current' && view && <option value="current">{view.label}</option>}
          {options.map(o => (
            <option key={o.id} value={o.id}>{o.label}</option>
          ))}
        </select>
        <span className="w-4 h-4 shrink-0" aria-hidden="true">
          {busy === 'menu' && <Loader2 className="w-4 h-4 animate-spin motion-reduce:animate-none text-shodh-text-faint" />}
        </span>
      </div>
      <p
        id={stateId}
        aria-live="polite"
        title={error ?? (slot.text || undefined)}
        className={cn('min-h-5 leading-5 text-caption truncate', error ? 'text-shodh-error' : 'text-shodh-text-muted')}
      >
        {error ? error : (
          <>
            {slot.word && <span className="font-medium text-shodh-text">{slot.word}</span>}
            {slot.word && slot.text ? ' ' : ''}
            {slot.text}
          </>
        )}
      </p>
      <div className="h-8 flex items-center gap-1.5" role="group" aria-label="Repeat actions">
        {actions.map(action => (
          <button
            key={action}
            type="button"
            disabled={busy !== null}
            onClick={() => act(action)}
            className={cn(ACTION, ACTION_WIDTH[action])}
          >
            {busy === action ? <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-label="Saving" /> : ACTION_LABELS[action]}
          </button>
        ))}
      </div>
    </div>
  );
}

/** The series' history, newest first: the last section of the sheet, so new rows never push a field. */
export function RepeatHistory({ repeat }: { repeat: RepeatState }) {
  const headingId = useId();
  const [all, setAll] = useState(false);
  if (!repeat.view) return null;
  const rows = repeat.runs.map(historyRow);
  const shown = all ? rows : rows.slice(0, HISTORY_SHOWN);
  return (
    <section aria-labelledby={headingId} className="flex flex-col gap-1">
      <h3 id={headingId} className={FIELD_LABEL}>History</h3>
      {rows.length === 0 ? (
        <p className="h-7 leading-7 text-caption text-shodh-text-faint">Nothing yet.</p>
      ) : (
        <ul className="flex flex-col">
          {shown.map(row => {
            const copy = row.copyTaskId;
            return (
            <li key={row.id} className="h-7 flex items-center gap-2 text-caption">
              <span className="w-20 shrink-0 text-shodh-text-muted tabular-nums">{row.date}</span>
              <span className="w-16 shrink-0 font-medium text-shodh-text">{row.word}</span>
              <span className="flex-1 min-w-0 truncate text-shodh-text-muted" title={row.reason}>{row.reason}</span>
              <span className="w-12 shrink-0 flex justify-end">
                {copy && (
                  <button
                    type="button"
                    onClick={() => openInTasks({ taskId: copy })}
                    className={cn('h-6 px-1.5 rounded-md text-shodh-accent-text hover:bg-shodh-raised', FOCUS_RING)}
                  >
                    Open
                  </button>
                )}
              </span>
            </li>
            );
          })}
        </ul>
      )}
      {rows.length > HISTORY_SHOWN && (
        <button
          type="button"
          onClick={() => setAll(v => !v)}
          className={cn('self-end h-6 px-1.5 rounded-md text-caption text-shodh-text-muted hover:bg-shodh-raised', FOCUS_RING)}
        >
          {all ? 'Show fewer' : `Show all (${rows.length})`}
        </button>
      )}
    </section>
  );
}
