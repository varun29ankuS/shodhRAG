import React, { useId, useState } from 'react';
import { Bell, BellOff, BellRing } from 'lucide-react';
import { cn } from '../../lib/utils';
import { DateTimeField, FIELD_LABEL, FOCUS_RING } from './fields';
import { nextRing, presetReminder, REMINDER_PRESETS, reminderChoice, reminderLabel, reminderStatus } from './reminders';
import type { TodoItem } from './types';

const SELECT = cn(
  'h-8 rounded-lg border border-shodh-border bg-shodh-surface px-2 text-[12.5px] text-shodh-text',
  'hover:border-shodh-border-strong focus:border-shodh-border-strong focus:outline-none focus-visible:ring-2 focus-visible:ring-ring',
);

/** Status line under the picker: when it rings, or that it already rang. */
function statusText(task: TodoItem): string | null {
  const ring = nextRing(task);
  switch (reminderStatus(task)) {
    case 'set':
      return ring ? `Rings ${reminderLabel(ring)}.` : null;
    case 'snoozed':
      return ring ? `Snoozed until ${reminderLabel(ring)}.` : null;
    case 'rang':
      return task.status === 'completed' ? 'Rang.' : 'Rang. Pick a time again to be reminded again.';
    default:
      return null;
  }
}

/**
 * Reminder picker for the task sheet: none, a preset relative to the due
 * time (needs a due date), or a custom date and time. Every choice saves at
 * once; the stored value is always the absolute local time.
 */
export function ReminderField({
  task,
  onChange,
}: {
  task: TodoItem;
  /** The new reminder (`YYYY-MM-DDTHH:MM`), or null to remove it. */
  onChange: (reminder: string | null) => void;
}) {
  const selectId = useId();
  const statusId = useId();
  const stored = reminderChoice(task.reminder, task.dueDate);
  // "Custom" stays open while the user picks a time, before anything is saved.
  const [customOpen, setCustomOpen] = useState(false);
  const choice = customOpen && stored === 'none' ? 'custom' : stored;
  const hasDue = REMINDER_PRESETS.some(p => presetReminder(task.dueDate, p) !== null);
  const status = statusText(task);
  const Icon = reminderStatus(task) === 'rang' ? BellOff : reminderStatus(task) === 'none' ? Bell : BellRing;

  const choose = (next: string) => {
    if (next === 'custom') {
      setCustomOpen(true);
      return;
    }
    setCustomOpen(false);
    if (next === 'none') {
      if (task.reminder) onChange(null);
      return;
    }
    const preset = REMINDER_PRESETS.find(p => p.id === next);
    const value = preset ? presetReminder(task.dueDate, preset) : null;
    if (value && value !== task.reminder) onChange(value);
  };

  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={selectId} className={cn(FIELD_LABEL, 'inline-flex items-center gap-1.5')}>
        <Icon className="w-3.5 h-3.5" aria-hidden="true" />
        Reminder
      </label>
      <select
        id={selectId}
        value={choice}
        aria-describedby={status ? statusId : undefined}
        onChange={e => choose(e.target.value)}
        className={cn(SELECT, 'self-start', FOCUS_RING)}
      >
        <option value="none">No reminder</option>
        {REMINDER_PRESETS.map(p => (
          <option key={p.id} value={p.id} disabled={!hasDue}>
            {p.label}{hasDue ? '' : ' (needs a due date)'}
          </option>
        ))}
        <option value="custom">Custom time…</option>
      </select>
      {choice === 'custom' && (
        <DateTimeField
          key={`reminder-${task.id}`}
          label="Remind at"
          value={task.reminder}
          requireTime
          onCommit={value => {
            setCustomOpen(false);
            if (value !== task.reminder) onChange(value);
          }}
          onClear={task.reminder ? () => { setCustomOpen(false); onChange(null); } : undefined}
          emptyHint="Pick a date and a time."
        />
      )}
      {status && (
        <p id={statusId} className="text-[11px] text-shodh-text-faint">
          {status} Reminders ring on this computer while Shodh is running, also from the tray.
        </p>
      )}
    </div>
  );
}

/** Small bell on task rows with a reminder still to ring. */
export function ReminderBadge({ task }: { task: TodoItem }) {
  const status = reminderStatus(task);
  const ring = nextRing(task);
  if (status === 'none' || status === 'rang' || !ring || task.status === 'completed') return null;
  const text = status === 'snoozed' ? `Snoozed until ${reminderLabel(ring)}` : `Reminder ${reminderLabel(ring)}`;
  return (
    <span
      className="h-6 px-1.5 inline-flex items-center gap-1 rounded-md text-[11.5px] text-shodh-text-muted"
      title={text}
    >
      <BellRing className="w-3 h-3" aria-hidden="true" />
      <span className="sr-only">{text}</span>
    </span>
  );
}
