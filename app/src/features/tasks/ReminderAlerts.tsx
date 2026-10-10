import React, { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { BellRing, Check, Clock, ExternalLink, History } from 'lucide-react';
import { toast } from 'sonner';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { publishTarget } from '../agent/navigation';
import { FOCUS_RING } from './fields';
import { reminderLabel } from './reminders';

/** Payload of `reminder-fired` / items of `reminders-missed` (`reminders.rs::DueReminder`). */
interface DueReminder {
  taskId: string;
  title: string;
  reminder: string;
  dueDate?: string | null;
}

const SNOOZE_MINUTES = 10;
const MISSED_TOAST_ID = 'reminders-missed';
/** Missed reminders named in the toast; the rest are counted. */
const MISSED_SHOWN = 5;

const ACTION = cn(
  'h-7 px-2.5 inline-flex items-center gap-1 rounded-md text-[12px] font-medium transition-colors duration-micro',
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
  publishTarget({ kind: 'calendar', date: null, taskId, eventId: null });
  window.dispatchEvent(new CustomEvent('switchTab', { detail: 'tasks' }));
}

function ReminderToast({ id, reminder }: { id: string | number; reminder: DueReminder }) {
  const act = (run: () => Promise<unknown>, failure: string) => {
    toast.dismiss(id);
    run().catch(err => notify.error(failure, { description: errorText(err) }));
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
          <span id={`${id}-title`} className="text-[13px] font-medium truncate">{reminder.title}</span>
          <span id={`${id}-when`} className="text-[11.5px] text-shodh-text-muted">
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
        <span id="reminders-missed-title" className="text-[13px] font-medium">
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
              <span className="block text-[12.5px] truncate">{r.title}</span>
              <span className="block text-[11px] text-shodh-text-muted">{reminderLabel(r.reminder)}</span>
            </button>
          </li>
        ))}
        {more > 0 && <li className="px-1 text-[11.5px] text-shodh-text-muted">and {more} more</li>}
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
        id: `reminder-${reminder.taskId}`,
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
