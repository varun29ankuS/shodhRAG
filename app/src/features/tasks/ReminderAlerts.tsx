import React, { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { BellRing, Check, Clock, ExternalLink, History } from 'lucide-react';
import { toast } from 'sonner';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { openInTasks } from './openInTasks';
import { FOCUS_RING } from './fields';
import { reminderLabel } from './reminders';
import { repeatDecide } from './repeatApi';
import type { RepeatChoice } from './repeat';

const REPEAT_ANSWERS: ReadonlyArray<{ choice: RepeatChoice; label: string }> = [
  { choice: 'make', label: 'Make the next one' },
  { choice: 'skip', label: 'Skip this one' },
  { choice: 'mark_done', label: 'Mark done' },
];

/** Payload of `reminder-fired` / items of `reminders-missed` (`reminders.rs::DueReminder`). */
interface DueReminder {
  taskId: string;
  title: string;
  reminder: string;
  dueDate?: string | null;
  /** A repeating task's question: the next one fell due while this copy is open. */
  repeat?: { jobId: string; occurrence: string; detail: string } | null;
}

const SNOOZE_MINUTES = 10;
const MISSED_TOAST_ID = 'reminders-missed';
/** Missed reminders named in the toast; the rest are counted. */
const MISSED_SHOWN = 5;

const ACTION = cn(
  'h-7 px-2.5 inline-flex items-center gap-1 rounded-md text-caption font-medium transition-colors duration-micro',
  FOCUS_RING,
);

function isDueReminder(value: unknown): value is DueReminder {
  if (typeof value !== 'object' || value === null) return false;
  const v = value as Record<string, unknown>;
  return typeof v.taskId === 'string' && typeof v.title === 'string' && typeof v.reminder === 'string';
}

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Show the task in Tasks with its sheet open. */
function openTask(taskId: string) {
  openInTasks({ taskId });
}

/** A repeating task's question: its three answers, no snooze (that would ring the open copy again). */
function RepeatToast({ id, reminder, jobId, detail }: { id: string | number; reminder: DueReminder; jobId: string; detail: string }) {
  const answer = (choice: RepeatChoice) => {
    toast.dismiss(id);
    repeatDecide(jobId, choice).catch(err => notify.error('The answer was not saved', { description: errorText(err) }));
  };
  return (
    <div
      role="alert"
      aria-labelledby={`${id}-title`}
      aria-describedby={`${id}-when`}
      className="w-[356px] max-w-full flex flex-col gap-2 p-3 rounded-[10px] border border-shodh-border-strong bg-shodh-surface text-shodh-text shadow-[0_8px_24px_rgba(0,0,0,0.3)]"
    >
      <div className="flex items-start gap-2">
        <BellRing className="w-4 h-4 mt-0.5 shrink-0 text-shodh-accent-text" aria-hidden="true" />
        <div className="min-w-0 flex flex-col">
          <span id={`${id}-title`} className="text-small font-medium truncate">{reminder.title}</span>
          <span id={`${id}-when`} className="text-caption text-shodh-text-muted">{detail}</span>
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-1.5 justify-end">
        {REPEAT_ANSWERS.map(({ choice, label }, i) => (
          <button
            key={choice}
            type="button"
            className={cn(ACTION, i === 0 ? 'bg-shodh-accent text-shodh-on-accent hover:opacity-90' : 'text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text')}
            onClick={() => answer(choice)}
          >
            {label}
          </button>
        ))}
        <button
          type="button"
          className={cn(ACTION, 'text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text')}
          onClick={() => {
            toast.dismiss(id);
            openTask(reminder.taskId);
          }}
        >
          <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" />
          Open
        </button>
      </div>
    </div>
  );
}

function ReminderToast({ id, reminder }: { id: string | number; reminder: DueReminder }) {
  const act = (run: () => Promise<unknown>, failure: string) => {
    toast.dismiss(id);
    run().catch(err => notify.error(failure, { description: errorText(err) }));
  };
  if (reminder.repeat) {
    return <RepeatToast id={id} reminder={reminder} jobId={reminder.repeat.jobId} detail={reminder.repeat.detail} />;
  }
  return (
    <div
      role="alert"
      aria-labelledby={`${id}-title`}
      aria-describedby={`${id}-when`}
      className="w-[356px] max-w-full flex flex-col gap-2 p-3 rounded-[10px] border border-shodh-border-strong bg-shodh-surface text-shodh-text shadow-[0_8px_24px_rgba(0,0,0,0.3)]"
    >
      <div className="flex items-start gap-2">
        <BellRing className="w-4 h-4 mt-0.5 shrink-0 text-shodh-accent-text" aria-hidden="true" />
        <div className="min-w-0 flex flex-col">
          <span id={`${id}-title`} className="text-small font-medium truncate">{reminder.title}</span>
          <span id={`${id}-when`} className="text-caption text-shodh-text-muted">
            Reminder · {reminderLabel(reminder.reminder)}
          </span>
        </div>
      </div>
      <div className="flex items-center gap-1.5 justify-end">
        <button
          type="button"
          className={cn(ACTION, 'text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text')}
          onClick={() => act(
            () => invoke('snooze_reminder', { taskId: reminder.taskId, minutes: SNOOZE_MINUTES }),
            'Could not snooze the reminder',
          )}
        >
          <Clock className="w-3.5 h-3.5" aria-hidden="true" />
          Snooze {SNOOZE_MINUTES} min
        </button>
        <button
          type="button"
          className={cn(ACTION, 'text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text')}
          onClick={() => act(
            () => invoke('update_task', { id: reminder.taskId, status: 'completed' }),
            'Could not mark the task done',
          )}
        >
          <Check className="w-3.5 h-3.5" aria-hidden="true" />
          Mark done
        </button>
        <button
          type="button"
          className={cn(ACTION, 'bg-shodh-accent text-shodh-on-accent hover:opacity-90')}
          onClick={() => {
            toast.dismiss(id);
            openTask(reminder.taskId);
          }}
        >
          <ExternalLink className="w-3.5 h-3.5" aria-hidden="true" />
          Open
        </button>
      </div>
    </div>
  );
}

function MissedToast({ reminders }: { reminders: DueReminder[] }) {
  const dismiss = () => {
    toast.dismiss(MISSED_TOAST_ID);
    invoke('dismiss_missed_reminders').catch(err => console.error('Could not dismiss missed reminders:', errorText(err)));
  };
  const shown = reminders.slice(0, MISSED_SHOWN);
  const more = reminders.length - shown.length;
  return (
    <div
      role="alert"
      aria-labelledby="reminders-missed-title"
      className="w-[356px] max-w-full flex flex-col gap-2 p-3 rounded-[10px] border border-shodh-border-strong bg-shodh-surface text-shodh-text shadow-[0_8px_24px_rgba(0,0,0,0.3)]"
    >
      <div className="flex items-start gap-2">
        <History className="w-4 h-4 mt-0.5 shrink-0 text-shodh-warning" aria-hidden="true" />
        <span id="reminders-missed-title" className="text-small font-medium">
          Missed while Shodh was closed
        </span>
      </div>
      <ul className="flex flex-col gap-0.5 pl-6">
        {shown.map(r => (
          <li key={`${r.taskId}-${r.reminder}`}>
            <button
              type="button"
              onClick={() => { dismiss(); openTask(r.taskId); }}
              className={cn('w-full text-left rounded px-1 py-0.5 hover:bg-shodh-raised', FOCUS_RING)}
            >
              <span className="block text-small truncate">{r.title}</span>
              <span className="block text-caption text-shodh-text-muted">{reminderLabel(r.reminder)}</span>
            </button>
          </li>
        ))}
        {more > 0 && <li className="px-1 text-caption text-shodh-text-muted">and {more} more</li>}
      </ul>
      <div className="flex justify-end">
        <button type="button" className={cn(ACTION, 'text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text')} onClick={dismiss}>
          Dismiss
        </button>
      </div>
    </div>
  );
}

function showMissed(value: unknown) {
  const reminders = Array.isArray(value) ? value.filter(isDueReminder) : [];
  if (reminders.length === 0) return;
  // One toast, replaced (same id) if the list arrives twice.
  toast.custom(() => <MissedToast reminders={reminders} />, { id: MISSED_TOAST_ID, duration: Infinity });
}

/**
 * In-app side of reminders. The native notification (Windows) carries only
 * a title and text, so the actions live here: a toast per reminder with
 * Snooze, Mark done and Open, kept until handled, plus one toast listing
 * reminders missed while the app was closed.
 */
export function ReminderAlerts() {
  useEffect(() => {
    let active = true;
    const unlisteners: (() => void)[] = [];
    const keep = (fn: () => void) => {
      if (active) unlisteners.push(fn);
      else fn();
    };
    listen<unknown>('reminder-fired', event => {
      if (!isDueReminder(event.payload)) return;
      const reminder = event.payload;
      toast.custom(id => <ReminderToast id={id} reminder={reminder} />, {
        // One toast per series' question, replaced when it moves to a newer occurrence.
        id: reminder.repeat ? `repeat-${reminder.repeat.jobId}` : `reminder-${reminder.taskId}`,
        duration: Infinity,
      });
    })
      .then(keep)
      .catch(err => console.error('Failed to listen for reminders:', err));
    listen<unknown>('reminders-missed', event => showMissed(event.payload))
      .then(keep)
      .catch(err => console.error('Failed to listen for missed reminders:', err));
    // The scheduler may have reported missed reminders before this mounted.
    invoke<unknown>('list_missed_reminders')
      .then(list => { if (active) showMissed(list); })
      .catch(err => console.error('Could not load missed reminders:', errorText(err)));
    return () => {
      active = false;
      for (const fn of unlisteners) fn();
    };
  }, []);
  return null;
}
