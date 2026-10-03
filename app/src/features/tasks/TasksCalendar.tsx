import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { AlertCircle, Bot, Check, ChevronLeft, ChevronRight, RotateCcw } from 'lucide-react';
import { cn } from '../../lib/utils';
import { notify } from '../../lib/notify';
import { addDays, dayKey, groupByDay, monthGrid } from './calendarGrid';
import type { GridDay } from './calendarGrid';

/** Wire shape of `load_tasks` (camelCase `TodoItem`); only the fields shown here. */
interface CalendarTask {
  id: string;
  title: string;
  dueDate: string | null;
  priority: string;
  status: string;
  source: string;
}

/** Wire shape of `load_events` (camelCase `CalendarEvent`); only the fields shown here. */
interface CalendarEntry {
  id: string;
  title: string;
  startTime: string;
  endTime: string | null;
  allDay: boolean;
  source: string;
}

const FOCUS_RING =
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-shodh-ground';

const ICON_BUTTON = cn(
  'w-8 h-8 rounded-lg inline-flex items-center justify-center text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
  FOCUS_RING,
);

/** Chips shown in a day cell before "+N more". */
const MAX_CHIPS = 3;

const WEEKDAYS = Array.from({ length: 7 }, (_, i) =>
  new Date(2026, 1, 1 + i).toLocaleDateString(undefined, { weekday: 'short' }),
);

function formatTime(iso: string): string {
  return new Date(iso).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
}

function isDone(task: CalendarTask): boolean {
  return task.status === 'completed';
}

/**
 * Tasks as a month calendar: tasks on their due day, events on their start
 * day. Arrow keys move between days; the selected day's items are listed
 * beside the grid, where tasks can be completed.
 */
export default function TasksCalendar() {
  const [tasks, setTasks] = useState<CalendarTask[]>([]);
  const [events, setEvents] = useState<CalendarEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Date>(() => new Date());
  const [month, setMonth] = useState(() => {
    const now = new Date();
    return { year: now.getFullYear(), month: now.getMonth() };
  });
  const gridRef = useRef<HTMLDivElement>(null);
  const focusAfterRender = useRef(false);

  const fetchData = useCallback(async () => {
    try {
      const [t, e] = await Promise.all([
        invoke<CalendarTask[]>('load_tasks'),
        invoke<CalendarEntry[]>('load_events'),
      ]);
      setTasks(t);
      setEvents(e);
      setError(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void fetchData();
  }, [fetchData]);

  // The backend emits after every task or event write, including the agent's.
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    listen('calendar-changed', () => { void fetchData(); })
      .then(fn => { if (active) unlisten = fn; else fn(); })
      .catch(err => console.error('Failed to listen for calendar changes:', err));
    return () => { active = false; unlisten?.(); };
  }, [fetchData]);

  const weeks = useMemo(() => monthGrid(month.year, month.month), [month]);
  const tasksByDay = useMemo(() => groupByDay(tasks, t => t.dueDate), [tasks]);
  const eventsByDay = useMemo(() => {
    const map = groupByDay(events, e => e.startTime);
    for (const list of map.values()) list.sort((a, b) => a.startTime.localeCompare(b.startTime));
    return map;
  }, [events]);

  const selectedKey = dayKey(selected);
  const todayKey = dayKey(new Date());
  const monthLabel = new Date(month.year, month.month, 1).toLocaleDateString(undefined, { month: 'long', year: 'numeric' });

  const select = useCallback((date: Date, moveFocus: boolean) => {
    setSelected(date);
    setMonth(prev =>
      prev.year === date.getFullYear() && prev.month === date.getMonth()
        ? prev
        : { year: date.getFullYear(), month: date.getMonth() },
    );
    focusAfterRender.current = moveFocus;
  }, []);

  // Keep keyboard focus on the selected day after arrow-key moves (which may change month).
  useEffect(() => {
    if (!focusAfterRender.current) return;
    focusAfterRender.current = false;
    gridRef.current?.querySelector<HTMLButtonElement>(`[data-day="${selectedKey}"]`)?.focus();
  }, [selectedKey, weeks]);

  const shiftMonth = (delta: number) => {
    const target = new Date(month.year, month.month + delta, 1);
    setMonth({ year: target.getFullYear(), month: target.getMonth() });
  };

  const handleGridKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const moves: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -7, ArrowDown: 7 };
    if (e.key in moves) {
      e.preventDefault();
      select(addDays(selected, moves[e.key]), true);
    } else if (e.key === 'PageUp' || e.key === 'PageDown') {
      e.preventDefault();
      const target = new Date(selected.getFullYear(), selected.getMonth() + (e.key === 'PageUp' ? -1 : 1), 1);
      const lastDay = new Date(target.getFullYear(), target.getMonth() + 1, 0).getDate();
      select(new Date(target.getFullYear(), target.getMonth(), Math.min(selected.getDate(), lastDay)), true);
    } else if (e.key === 'Home') {
      e.preventDefault();
      select(addDays(selected, -selected.getDay()), true);
    } else if (e.key === 'End') {
      e.preventDefault();
      select(addDays(selected, 6 - selected.getDay()), true);
    }
  };

  const toggleTask = async (task: CalendarTask) => {
    const status = isDone(task) ? 'pending' : 'completed';
    setTasks(prev => prev.map(t => (t.id === task.id ? { ...t, status } : t)));
    try {
      await invoke('update_task', { id: task.id, status });
    } catch (err) {
      setTasks(prev => prev.map(t => (t.id === task.id ? task : t)));
      notify.error('Could not update the task', { description: String(err) });
    }
  };

  if (error && tasks.length === 0 && events.length === 0) {
    return (
      <div className="h-full flex items-center justify-center p-6">
        <div role="alert" className="max-w-sm text-center flex flex-col items-center gap-3">
          <AlertCircle className="w-6 h-6 text-shodh-error" aria-hidden="true" />
          <p className="text-sm text-shodh-text-secondary">Could not load tasks and events.</p>
          <p className="text-[12px] text-shodh-text-faint break-words">{error}</p>
          <button
            type="button"
            onClick={() => { setLoading(true); void fetchData(); }}
            className={cn('h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border text-[12.5px] hover:bg-shodh-raised', FOCUS_RING)}
          >
            <RotateCcw className="w-3.5 h-3.5" aria-hidden="true" />
            Try again
          </button>
        </div>
      </div>
    );
  }

  const dayTasks = tasksByDay.get(selectedKey) ?? [];
  const dayEvents = eventsByDay.get(selectedKey) ?? [];
  const selectedLabel = selected.toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric' });

  return (
    <div className="h-full flex gap-5 px-6 pb-6 pt-2 min-h-0">
      <section aria-labelledby="tasks-calendar-month" className="flex-1 min-w-0 flex flex-col min-h-0">
        <div className="flex items-center gap-1 mb-3 shrink-0">
          <h2 id="tasks-calendar-month" className="text-[15px] font-semibold mr-auto" aria-live="polite">
            {monthLabel}
          </h2>
          <button
            type="button"
            onClick={() => select(new Date(), false)}
            className={cn('h-8 px-3 rounded-lg border border-shodh-border text-[12.5px] text-shodh-text-secondary hover:bg-shodh-raised transition-colors duration-micro', FOCUS_RING)}
          >
            Today
          </button>
          <button type="button" onClick={() => shiftMonth(-1)} aria-label="Previous month" className={ICON_BUTTON}>
            <ChevronLeft className="w-4 h-4" aria-hidden="true" />
          </button>
          <button type="button" onClick={() => shiftMonth(1)} aria-label="Next month" className={ICON_BUTTON}>
            <ChevronRight className="w-4 h-4" aria-hidden="true" />
          </button>
        </div>

        <div
          ref={gridRef}
          role="grid"
          aria-labelledby="tasks-calendar-month"
          aria-busy={loading}
          onKeyDown={handleGridKeyDown}
          className="flex-1 min-h-0 grid grid-rows-[auto_repeat(6,minmax(0,1fr))] rounded-xl border border-shodh-border overflow-hidden bg-shodh-surface"
        >
          <div role="row" className="grid grid-cols-7 border-b border-shodh-border">
            {WEEKDAYS.map(d => (
              <div key={d} role="columnheader" className="px-2 py-1.5 text-[11px] font-medium text-shodh-text-faint">
                {d}
              </div>
            ))}
          </div>
          {weeks.map((week, w) => (
            <div key={week[0].key} role="row" className={cn('grid grid-cols-7 min-h-0', w < 5 && 'border-b border-shodh-border-subtle')}>
              {week.map(day => (
                <DayCell
                  key={day.key}
                  day={day}
                  loading={loading}
                  isToday={day.key === todayKey}
                  isSelected={day.key === selectedKey}
                  tasks={tasksByDay.get(day.key) ?? []}
                  events={eventsByDay.get(day.key) ?? []}
                  onSelect={() => select(day.date, false)}
                />
              ))}
            </div>
          ))}
        </div>
      </section>

      <aside aria-labelledby="tasks-calendar-day" className="w-72 shrink-0 flex flex-col min-h-0">
        <h2 id="tasks-calendar-day" className="text-[15px] font-semibold mb-3 h-8 flex items-center">
          {selectedLabel}
        </h2>
        <div className="flex-1 min-h-0 overflow-y-auto scrollbar-thin flex flex-col gap-4">
          {dayEvents.length === 0 && dayTasks.length === 0 ? (
            <p className="text-[12.5px] text-shodh-text-faint">
              Nothing on this day. Ask Shodh to schedule something, or add a task with a due date from the list view.
            </p>
          ) : null}
          {dayEvents.length > 0 && (
            <div className="flex flex-col gap-1.5">
              <h3 className="text-[11px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">Events</h3>
              <ul className="flex flex-col gap-1.5">
                {dayEvents.map(ev => (
                  <li key={ev.id} className="flex items-start gap-2 px-3 py-2 rounded-lg bg-shodh-surface border border-shodh-border">
                    <span className="mt-1 w-1.5 h-1.5 rounded-full bg-shodh-info shrink-0" aria-hidden="true" />
                    <span className="flex-1 min-w-0 flex flex-col">
                      <span className="text-[13px] text-shodh-text truncate">{ev.title}</span>
                      <span className="text-[11.5px] text-shodh-text-faint">
                        {ev.allDay ? 'All day' : `${formatTime(ev.startTime)}${ev.endTime ? ` – ${formatTime(ev.endTime)}` : ''}`}
                      </span>
                    </span>
                    {ev.source === 'agent' && (
                      <Bot className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-text-muted" aria-label="Created by the assistant" />
                    )}
                  </li>
                ))}
              </ul>
            </div>
          )}
          {dayTasks.length > 0 && (
            <div className="flex flex-col gap-1.5">
              <h3 className="text-[11px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">Due</h3>
              <ul className="flex flex-col gap-1.5">
                {dayTasks.map(task => (
                  <li key={task.id} className="flex items-start gap-2 px-3 py-2 rounded-lg bg-shodh-surface border border-shodh-border">
                    <button
                      type="button"
                      role="checkbox"
                      aria-checked={isDone(task)}
                      aria-label={`${isDone(task) ? 'Mark not done' : 'Mark done'}: ${task.title}`}
                      onClick={() => void toggleTask(task)}
                      className={cn(
                        'mt-0.5 w-4 h-4 shrink-0 rounded border inline-flex items-center justify-center transition-colors duration-micro',
                        isDone(task) ? 'bg-shodh-success border-shodh-success text-shodh-ground' : 'border-shodh-border-strong hover:border-shodh-text-muted',
                        FOCUS_RING,
                      )}
                    >
                      {isDone(task) && <Check className="w-3 h-3" strokeWidth={3} aria-hidden="true" />}
                    </button>
                    <span className="flex-1 min-w-0 flex flex-col">
                      <span className={cn('text-[13px] truncate', isDone(task) ? 'line-through text-shodh-text-faint' : 'text-shodh-text')}>
                        {task.title}
                      </span>
                      <span className="text-[11.5px] text-shodh-text-faint capitalize">{task.priority} priority</span>
                    </span>
                    {task.source === 'agent' && (
                      <Bot className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-text-muted" aria-label="Created by the assistant" />
                    )}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      </aside>
    </div>
  );
}

function DayCell({
  day,
  loading,
  isToday,
  isSelected,
  tasks,
  events,
  onSelect,
}: {
  day: GridDay;
  loading: boolean;
  isToday: boolean;
  isSelected: boolean;
  tasks: CalendarTask[];
  events: CalendarEntry[];
  onSelect: () => void;
}) {
  const chips = [
    ...events.map(e => ({ id: `e-${e.id}`, title: e.allDay ? e.title : `${formatTime(e.startTime)} ${e.title}`, kind: 'event' as const, done: false })),
    ...tasks.map(t => ({ id: `t-${t.id}`, title: t.title, kind: 'task' as const, done: isDone(t) })),
  ];
  const shown = chips.slice(0, MAX_CHIPS);
  const more = chips.length - shown.length;
  const dateLabel = day.date.toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric' });
  const summary = [
    events.length > 0 ? `${events.length} event${events.length === 1 ? '' : 's'}` : null,
    tasks.length > 0 ? `${tasks.length} task${tasks.length === 1 ? '' : 's'} due` : null,
  ].filter(Boolean).join(', ');

  return (
    <div role="gridcell" aria-selected={isSelected} className="min-w-0 min-h-0 border-r border-shodh-border-subtle last:border-r-0">
      <button
        type="button"
        data-day={day.key}
        tabIndex={isSelected ? 0 : -1}
        onClick={onSelect}
        aria-label={`${dateLabel}${isToday ? ', today' : ''}${summary ? `, ${summary}` : ''}`}
        aria-current={isToday ? 'date' : undefined}
        className={cn(
          'w-full h-full flex flex-col items-stretch gap-0.5 p-1.5 text-left transition-colors duration-micro focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring',
          isSelected ? 'bg-shodh-accent-soft' : 'hover:bg-shodh-raised/60',
          !day.inMonth && 'opacity-60',
        )}
      >
        <span
          className={cn(
            'self-start text-[11.5px] tabular-nums w-6 h-6 inline-flex items-center justify-center rounded-full',
            isToday ? 'bg-shodh-accent text-shodh-on-accent font-semibold' : day.inMonth ? 'text-shodh-text-secondary' : 'text-shodh-text-faint',
          )}
          aria-hidden="true"
        >
          {day.date.getDate()}
        </span>
        {loading ? (
          day.inMonth && day.date.getDate() % 4 === 1 ? (
            <span className="shell-skeleton h-3.5 rounded w-4/5" aria-hidden="true" />
          ) : null
        ) : (
          <span className="flex flex-col gap-0.5 min-h-0 overflow-hidden" aria-hidden="true">
            {shown.map(chip => (
              <span
                key={chip.id}
                className={cn(
                  'truncate text-[11px] leading-4 px-1.5 rounded',
                  chip.kind === 'event' ? 'bg-shodh-info/15 text-shodh-text' : 'bg-shodh-raised-2 text-shodh-text-secondary',
                  chip.done && 'line-through text-shodh-text-faint',
                )}
              >
                {chip.title}
              </span>
            ))}
            {more > 0 && <span className="text-[10.5px] px-1.5 text-shodh-text-faint">+{more} more</span>}
          </span>
        )}
      </button>
    </div>
  );
}
