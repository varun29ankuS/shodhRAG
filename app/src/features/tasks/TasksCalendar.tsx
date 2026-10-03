import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AlertCircle, Bot, Check, ChevronLeft, ChevronRight, GripVertical, Plus, RotateCcw } from 'lucide-react';
import { cn } from '../../lib/utils';
import { addDays, dayKey, groupByDay, monthGrid } from './calendarGrid';
import type { GridDay } from './calendarGrid';
import { fromInputs, parseMoment, rescheduleTo, storedDayKey, storedTime } from './dueDate';
import { useTasksStore } from './TasksStore';
import { isDone } from './types';
import type { CalendarEvent as CalendarEntry, TodoItem as CalendarTask } from './types';

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

function formatTime(value: string): string {
  const m = parseMoment(value);
  if (!m || m.kind === 'date') return '';
  return new Date(m.year, m.month - 1, m.day, m.hour, m.minute).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
}

/** "9:00 AM – 10:00 AM", or "All day" for all-day and date-only events. */
function eventTimeLabel(ev: CalendarEntry): string {
  const start = ev.allDay ? '' : formatTime(ev.startTime);
  if (!start) return 'All day';
  const end = ev.endTime ? formatTime(ev.endTime) : '';
  return end ? `${start} – ${end}` : start;
}

/** Pointer travel before a press becomes a drag, so plain clicks still select. */
const DRAG_THRESHOLD_PX = 4;

interface DragState {
  taskId: string;
  title: string;
  fromDay: string;
  startX: number;
  startY: number;
  x: number;
  y: number;
  active: boolean;
  overDay: string | null;
}

/**
 * Drag a task to another day with the pointer. Built on pointer events, not
 * HTML5 drag and drop, because the Tauri window handles OS file drops (the
 * Library relies on that), which stops HTML5 drop events in the webview.
 */
function useTaskDrag(onDrop: (taskId: string, toDay: string) => void) {
  const [drag, setDrag] = useState<DragState | null>(null);
  const dragRef = useRef<DragState | null>(null);
  const suppressClick = useRef(false);

  const update = (next: DragState | null) => {
    dragRef.current = next;
    setDrag(next);
  };

  const dragging = drag !== null;
  useEffect(() => {
    if (!dragging) return;
    const dayAt = (x: number, y: number) =>
      document.elementFromPoint(x, y)?.closest<HTMLElement>('[data-day]')?.dataset.day ?? null;
    const onMove = (e: PointerEvent) => {
      const cur = dragRef.current;
      if (!cur) return;
      const moved = Math.hypot(e.clientX - cur.startX, e.clientY - cur.startY) >= DRAG_THRESHOLD_PX;
      const active = cur.active || moved;
      update({ ...cur, x: e.clientX, y: e.clientY, active, overDay: active ? dayAt(e.clientX, e.clientY) : null });
    };
    const onUp = (e: PointerEvent) => {
      const cur = dragRef.current;
      update(null);
      if (!cur?.active) return;
      // The release ends a drag; it must not also count as a click on the day under it.
      suppressClick.current = true;
      window.setTimeout(() => { suppressClick.current = false; }, 0);
      const target = dayAt(e.clientX, e.clientY);
      if (target && target !== cur.fromDay) onDrop(cur.taskId, target);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        update(null);
      }
    };
    const cancel = () => update(null);
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    window.addEventListener('pointercancel', cancel);
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('blur', cancel);
    return () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      window.removeEventListener('pointercancel', cancel);
      window.removeEventListener('keydown', onKey, true);
      window.removeEventListener('blur', cancel);
    };
    // Listeners read the latest state from dragRef, so they bind once per drag.
  }, [dragging, onDrop]);

  const start = (e: React.PointerEvent, task: CalendarTask) => {
    if (e.button !== 0 || !e.isPrimary) return;
    const fromDay = storedDayKey(task.dueDate);
    if (!fromDay) return;
    update({ taskId: task.id, title: task.title, fromDay, startX: e.clientX, startY: e.clientY, x: e.clientX, y: e.clientY, active: false, overDay: null });
  };

  return { drag: drag?.active ? drag : null, start, suppressClick };
}

/**
 * Tasks as a month calendar: tasks on their due day, events on their start
 * day. Arrow keys move between days; the selected day's items are listed
 * beside the grid, where tasks can be completed and items opened for
 * editing. Tasks can be dragged to another day (the detail sheet's due date
 * is the keyboard route).
 */
type ComposerKind = 'task' | 'event';

/**
 * Adds a task due on, or an event on, the selected day. Events default to
 * 09:00–10:00; "All day" stores day-only start/end.
 */
function DayComposer({ dayKeyValue, inputRef }: { dayKeyValue: string; inputRef: React.RefObject<HTMLInputElement> }) {
  const { createTask, createEvent } = useTasksStore();
  const [kind, setKind] = useState<ComposerKind>('task');
  const [title, setTitle] = useState('');
  const [allDay, setAllDay] = useState(false);
  const [start, setStart] = useState('09:00');
  const [end, setEnd] = useState('10:00');
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const text = title.trim();
    if (!text || busy) return;
    setProblem(null);
    let ok = false;
    setBusy(true);
    if (kind === 'task') {
      ok = (await createTask({ title: text, dueDate: fromInputs(dayKeyValue, '') })) !== null;
    } else {
      const startTime = fromInputs(dayKeyValue, allDay ? '' : start);
      const endTime = allDay ? null : fromInputs(dayKeyValue, end);
      if (!startTime || (!allDay && !endTime)) {
        setProblem('Enter a valid start and end time.');
      } else if (!allDay && endTime && (storedTime(endTime) ?? 0) <= (storedTime(startTime) ?? 0)) {
        setProblem('The event must end after it starts.');
      } else {
        ok = (await createEvent({ title: text, startTime, endTime, allDay })) !== null;
      }
    }
    setBusy(false);
    if (ok) setTitle('');
    inputRef.current?.focus();
  };

  const control = 'h-8 rounded-lg border border-shodh-border bg-shodh-surface px-2 text-[12.5px] text-shodh-text hover:border-shodh-border-strong focus:outline-none focus-visible:ring-2 focus-visible:ring-ring';
  const segment = (value: ComposerKind, label: string) => (
    <button
      type="button"
      aria-pressed={kind === value}
      onClick={() => { setKind(value); setProblem(null); }}
      className={cn(
        'flex-1 h-7 rounded-md text-[12px] transition-colors duration-micro',
        kind === value ? 'bg-shodh-surface text-shodh-text shadow-sm' : 'text-shodh-text-muted hover:text-shodh-text',
        FOCUS_RING,
      )}
    >
      {label}
    </button>
  );

  return (
    <form onSubmit={submit} aria-label={kind === 'task' ? 'Add a task for this day' : 'Add an event on this day'} className="shrink-0 flex flex-col gap-2 p-2.5 mb-4 rounded-xl border border-shodh-border bg-shodh-raised">
      <div className="flex gap-1 p-0.5 rounded-lg bg-shodh-raised-2" role="group" aria-label="What to add">
        {segment('task', 'Task')}
        {segment('event', 'Event')}
      </div>
      <input
        ref={inputRef}
        type="text"
        value={title}
        onChange={e => setTitle(e.target.value)}
        onKeyDown={e => { if (e.key === 'Escape' && title) { e.preventDefault(); setTitle(''); } }}
        placeholder={kind === 'task' ? 'Task due this day' : 'Event title'}
        aria-label={kind === 'task' ? 'Task title' : 'Event title'}
        className={cn(control, 'w-full')}
      />
      {kind === 'event' && (
        <div className="flex items-center gap-2 flex-wrap">
          <label className="inline-flex items-center gap-1.5 text-[12px] text-shodh-text-secondary">
            <input type="checkbox" checked={allDay} onChange={e => setAllDay(e.target.checked)} className="accent-[var(--c-accent)]" />
            All day
          </label>
          {!allDay && (
            <>
              <input type="time" aria-label="Starts" value={start} onChange={e => setStart(e.target.value)} className={cn(control, 'w-[6.5rem] tabular-nums')} />
              <span className="text-[12px] text-shodh-text-faint" aria-hidden="true">–</span>
              <input type="time" aria-label="Ends" value={end} onChange={e => setEnd(e.target.value)} className={cn(control, 'w-[6.5rem] tabular-nums')} />
            </>
          )}
        </div>
      )}
      {problem && <p role="alert" className="text-[12px] text-shodh-error">{problem}</p>}
      <button
        type="submit"
        disabled={!title.trim() || busy}
        className={cn('h-8 rounded-lg bg-shodh-accent text-shodh-on-accent text-[12.5px] font-medium hover:bg-shodh-accent-hover disabled:opacity-50 transition-colors duration-micro inline-flex items-center justify-center gap-1.5', FOCUS_RING)}
      >
        <Plus className="w-3.5 h-3.5" aria-hidden="true" />
        {kind === 'task' ? 'Add task' : 'Add event'}
      </button>
    </form>
  );
}

export default function TasksCalendar() {
  const { tasks, events, loading, error, refresh, updateTask, openTask, openEvent } = useTasksStore();
  const [announcement, setAnnouncement] = useState('');
  const [selected, setSelected] = useState<Date>(() => new Date());
  const [month, setMonth] = useState(() => {
    const now = new Date();
    return { year: now.getFullYear(), month: now.getMonth() };
  });
  const gridRef = useRef<HTMLDivElement>(null);
  const composerRef = useRef<HTMLInputElement>(null);
  const focusAfterRender = useRef(false);

  const reschedule = useCallback((taskId: string, toDay: string) => {
    const task = tasks.find(t => t.id === taskId);
    const next = task?.dueDate ? rescheduleTo(task.dueDate, toDay) : null;
    if (!task || !next) return;
    void updateTask(taskId, { dueDate: next });
    const [y, m, d] = toDay.split('-').map(Number);
    setAnnouncement(`Moved “${task.title}” to ${new Date(y, m - 1, d).toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric' })}.`);
  }, [tasks, updateTask]);

  const { drag, start: startDrag, suppressClick } = useTaskDrag(reschedule);

  const weeks = useMemo(() => monthGrid(month.year, month.month), [month]);
  const tasksByDay = useMemo(() => groupByDay(tasks, t => t.dueDate), [tasks]);
  const eventsByDay = useMemo(() => {
    const map = groupByDay(events, e => e.startTime);
    for (const list of map.values()) list.sort((a, b) => (storedTime(a.startTime) ?? 0) - (storedTime(b.startTime) ?? 0));
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
    } else if (e.key === 'Enter' || e.key === 'n') {
      // Enter / N on a day: add something to it.
      e.preventDefault();
      composerRef.current?.focus();
    }
  };

  const toggleTask = (task: CalendarTask) => {
    void updateTask(task.id, { status: isDone(task) ? 'pending' : 'completed' });
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
            onClick={() => void refresh()}
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
                  isDropTarget={drag !== null && drag.overDay === day.key && drag.fromDay !== day.key}
                  onTaskPointerDown={startDrag}
                  onSelect={() => { if (!suppressClick.current) select(day.date, false); }}
                  onAdd={() => { select(day.date, false); window.requestAnimationFrame(() => composerRef.current?.focus()); }}
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
        <DayComposer key={selectedKey} dayKeyValue={selectedKey} inputRef={composerRef} />
        <div className="flex-1 min-h-0 overflow-y-auto scrollbar-thin flex flex-col gap-4">
          {dayEvents.length === 0 && dayTasks.length === 0 ? (
            <p className="text-[12.5px] text-shodh-text-faint">
              Nothing on this day yet. Double-click a day (or press Enter on it) to add something, or ask Shodh to schedule it.
            </p>
          ) : null}
          {dayTasks.length > 0 && (
            <p className="text-[11.5px] text-shodh-text-faint">
              Drag a task onto another day to move it, or open it to change its due date.
            </p>
          )}
          {dayEvents.length > 0 && (
            <div className="flex flex-col gap-1.5">
              <h3 className="text-[11px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint">Events</h3>
              <ul className="flex flex-col gap-1.5">
                {dayEvents.map(ev => (
                  <li key={ev.id}>
                    <button
                      type="button"
                      onClick={() => openEvent(ev.id)}
                      className={cn('w-full flex items-start gap-2 px-3 py-2 rounded-lg bg-shodh-surface border border-shodh-border text-left hover:bg-shodh-raised transition-colors duration-micro', FOCUS_RING)}
                    >
                      <span className="mt-1.5 w-1.5 h-1.5 rounded-full bg-shodh-info shrink-0" aria-hidden="true" />
                      <span className="flex-1 min-w-0 flex flex-col">
                        <span className="text-[13px] text-shodh-text truncate">{ev.title}</span>
                        <span className="text-[11.5px] text-shodh-text-faint">{eventTimeLabel(ev)}</span>
                      </span>
                      {ev.source === 'agent' && (
                        <Bot className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-text-muted" aria-label="Created by agent" />
                      )}
                    </button>
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
                    <button
                      type="button"
                      onClick={() => { if (!suppressClick.current) openTask(task.id); }}
                      onPointerDown={e => startDrag(e, task)}
                      aria-label={`${task.title}, ${task.priority} priority. Open details`}
                      className={cn('flex-1 min-w-0 flex flex-col text-left rounded-sm cursor-grab active:cursor-grabbing select-none', FOCUS_RING)}
                    >
                      <span className={cn('text-[13px] truncate', isDone(task) ? 'line-through text-shodh-text-faint' : 'text-shodh-text')}>
                        {task.title}
                      </span>
                      <span className="text-[11.5px] text-shodh-text-faint capitalize">{task.priority} priority</span>
                    </button>
                    {task.source === 'agent' && (
                      <Bot className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-text-muted" aria-label="Created by agent" />
                    )}
                    <GripVertical className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-text-faint" aria-hidden="true" />
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      </aside>

      <p className="sr-only" role="status" aria-live="polite">{announcement}</p>
      {drag && (
        <div
          className="fixed z-50 pointer-events-none max-w-[220px] truncate px-2 py-1 rounded-md border border-shodh-border-strong bg-shodh-surface text-[12px] text-shodh-text shadow-[0_8px_24px_rgba(0,0,0,0.3)]"
          style={{ left: drag.x + 12, top: drag.y + 8 }}
          aria-hidden="true"
        >
          {drag.title}
        </div>
      )}
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
  isDropTarget,
  onTaskPointerDown,
  onSelect,
  onAdd,
}: {
  day: GridDay;
  loading: boolean;
  isToday: boolean;
  isSelected: boolean;
  tasks: CalendarTask[];
  events: CalendarEntry[];
  isDropTarget: boolean;
  onTaskPointerDown: (e: React.PointerEvent, task: CalendarTask) => void;
  onSelect: () => void;
  onAdd: () => void;
}) {
  const chips = [
    ...events.map(e => {
      const time = e.allDay ? '' : formatTime(e.startTime);
      return { id: `e-${e.id}`, title: time ? `${time} ${e.title}` : e.title, kind: 'event' as const, done: false, task: null as CalendarTask | null };
    }),
    ...tasks.map(t => ({ id: `t-${t.id}`, title: t.title, kind: 'task' as const, done: isDone(t), task: t as CalendarTask | null })),
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
        onDoubleClick={onAdd}
        aria-label={`${dateLabel}${isToday ? ', today' : ''}${summary ? `, ${summary}` : ''}`}
        aria-current={isToday ? 'date' : undefined}
        className={cn(
          'w-full h-full flex flex-col items-stretch gap-0.5 p-1.5 text-left transition-colors duration-micro focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring',
          isSelected ? 'bg-shodh-accent-soft' : 'hover:bg-shodh-raised/60',
          !day.inMonth && 'opacity-60',
          isDropTarget && 'ring-2 ring-inset ring-shodh-accent-text bg-shodh-accent-soft',
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
                onPointerDown={chip.task ? e => onTaskPointerDown(e, chip.task as CalendarTask) : undefined}
                className={cn(
                  'truncate text-[11px] leading-4 px-1.5 rounded select-none',
                  chip.kind === 'event' ? 'bg-shodh-info/15 text-shodh-text' : 'bg-shodh-raised-2 text-shodh-text-secondary cursor-grab',
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
