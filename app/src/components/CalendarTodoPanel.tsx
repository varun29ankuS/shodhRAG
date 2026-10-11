import React, { useEffect, useId, useMemo, useRef, useState } from 'react';
import {
  AlertCircle, Bot, Check, CheckCircle2, ChevronLeft, ChevronRight, FileText, FolderOpen,
  ListTodo, Loader2, Plus, RotateCcw, Trash2, X,
} from 'lucide-react';
import { cn } from '../lib/utils';
import { dayKey, monthGrid } from '../features/tasks/calendarGrid';
import { fromInputs, isOverdue, storedDayKey, storedTime } from '../features/tasks/dueDate';
import { FOCUS_RING } from '../features/tasks/fields';
import { DueMenu, PriorityMenu } from '../features/tasks/QuickMenus';
import { ReminderBadge } from '../features/tasks/ReminderField';
import { useTasksStore } from '../features/tasks/TasksStore';
import { isDone, PRIORITIES, PRIORITY_LABELS } from '../features/tasks/types';
import type { CalendarEvent, TodoItem } from '../features/tasks/types';

type FilterTab = 'all' | 'pending' | 'completed';

/** A click on a title waits this long for a second click (double-click edits the title). */
const TITLE_CLICK_DELAY_MS = 220;

const ICON_BUTTON = cn(
  'w-7 h-7 rounded-lg inline-flex items-center justify-center text-shodh-text-muted hover:bg-shodh-raised hover:text-shodh-text transition-colors duration-micro',
  FOCUS_RING,
);

function sortTasks(list: TodoItem[]): TodoItem[] {
  return [...list].sort((a, b) => {
    if (isDone(a) !== isDone(b)) return isDone(a) ? 1 : -1;
    const aOverdue = !isDone(a) && isOverdue(a.dueDate);
    const bOverdue = !isDone(b) && isOverdue(b.dueDate);
    if (aOverdue !== bOverdue) return aOverdue ? -1 : 1;
    const at = storedTime(a.dueDate);
    const bt = storedTime(b.dueDate);
    if (at !== null && bt !== null && at !== bt) return at - bt;
    if (at !== null && bt === null) return -1;
    if (at === null && bt !== null) return 1;
    return (Date.parse(b.createdAt) || 0) - (Date.parse(a.createdAt) || 0);
  });
}

function formatEventWhen(event: CalendarEvent): string {
  const day = storedDayKey(event.startTime);
  const t = storedTime(event.startTime);
  if (!day || t === null) return event.startTime;
  const date = new Date(t);
  const dayText = date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  return event.allDay ? `${dayText} · All day` : `${dayText} · ${date.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })}`;
}

// ── Quick add ────────────────────────────────────────────────────

function QuickAdd({ defaultDay }: { defaultDay: string | null }) {
  const { createTask } = useTasksStore();
  const [title, setTitle] = useState('');
  const [date, setDate] = useState(defaultDay ?? '');
  const [priority, setPriority] = useState('medium');
  const [busy, setBusy] = useState(false);
  const titleRef = useRef<HTMLInputElement>(null);
  const titleId = useId();

  // Filtering by a day pre-fills it, unless a date was already chosen.
  useEffect(() => {
    if (defaultDay) setDate(prev => prev || defaultDay);
  }, [defaultDay]);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const text = title.trim();
    if (!text || busy) return;
    setBusy(true);
    const created = await createTask({ title: text, dueDate: date ? fromInputs(date, '') : null, priority });
    setBusy(false);
    if (created) {
      setTitle('');
      setDate(defaultDay ?? '');
      setPriority('medium');
    }
    titleRef.current?.focus();
  };

  const control = 'h-8 rounded-lg border border-shodh-border bg-shodh-surface px-2 text-[12.5px] text-shodh-text hover:border-shodh-border-strong focus:outline-none focus-visible:ring-2 focus-visible:ring-ring';

  return (
    <form onSubmit={submit} aria-label="Add a task" className="flex items-center gap-2 p-1.5 pl-3 rounded-xl border border-shodh-border bg-shodh-raised">
      <Plus className="w-4 h-4 shrink-0 text-shodh-text-faint" aria-hidden="true" />
      <label htmlFor={titleId} className="sr-only">New task</label>
      <input
        ref={titleRef}
        id={titleId}
        type="text"
        value={title}
        onChange={e => setTitle(e.target.value)}
        onKeyDown={e => { if (e.key === 'Escape' && title) { e.preventDefault(); setTitle(''); } }}
        placeholder="Add a task"
        className="flex-1 min-w-0 h-8 bg-transparent text-[13px] text-shodh-text placeholder:text-shodh-text-faint focus:outline-none"
      />
      <input
        type="date"
        aria-label="Due date (optional)"
        value={date}
        onChange={e => setDate(e.target.value)}
        className={cn(control, 'w-[9.5rem] tabular-nums')}
      />
      <select aria-label="Priority" value={priority} onChange={e => setPriority(e.target.value)} className={cn(control, 'w-[6.5rem]')}>
        {PRIORITIES.map(p => <option key={p} value={p}>{PRIORITY_LABELS[p]}</option>)}
      </select>
      <button
        type="submit"
        disabled={!title.trim() || busy}
        className={cn('h-8 px-3 rounded-lg bg-shodh-accent text-shodh-on-accent text-[12.5px] font-medium hover:bg-shodh-accent-hover disabled:opacity-50 transition-colors duration-micro inline-flex items-center gap-1.5', FOCUS_RING)}
      >
        {busy && <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
        Add
      </button>
    </form>
  );
}

// ── Task row ─────────────────────────────────────────────────────

function TaskRow({
  task,
  active,
  editing,
  onFocusRow,
  onStartEdit,
  onEndEdit,
  onRequestDelete,
  onMove,
}: {
  task: TodoItem;
  active: boolean;
  editing: boolean;
  onFocusRow: () => void;
  onStartEdit: () => void;
  onEndEdit: () => void;
  onRequestDelete: () => void;
  onMove: (to: 'prev' | 'next' | 'first' | 'last') => void;
}) {
  const { updateTask, openTask } = useTasksStore();
  const done = isDone(task);
  const overdue = !done && isOverdue(task.dueDate);
  const subDone = task.subtasks.filter(s => s.completed).length;
  const clickTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const rowRef = useRef<HTMLDivElement>(null);
  const [draft, setDraft] = useState(task.title);
  const cancelEdit = useRef(false);
  const inner = active ? 0 : -1;

  useEffect(() => () => { if (clickTimer.current) clearTimeout(clickTimer.current); }, []);
  useEffect(() => { if (editing) { setDraft(task.title); cancelEdit.current = false; } }, [editing, task.title]);

  const toggle = () => void updateTask(task.id, { status: done ? 'pending' : 'completed' });

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.target !== e.currentTarget) return;
    switch (e.key) {
      case 'Enter': e.preventDefault(); openTask(task.id); break;
      case ' ': e.preventDefault(); toggle(); break;
      case 'F2': e.preventDefault(); onStartEdit(); break;
      case 'Delete': e.preventDefault(); onRequestDelete(); break;
      case 'ArrowDown': e.preventDefault(); onMove('next'); break;
      case 'ArrowUp': e.preventDefault(); onMove('prev'); break;
      case 'Home': e.preventDefault(); onMove('first'); break;
      case 'End': e.preventDefault(); onMove('last'); break;
      default: break;
    }
  };

  const finishEdit = () => {
    const text = draft.trim();
    if (!cancelEdit.current && text && text !== task.title) void updateTask(task.id, { title: text });
    cancelEdit.current = false;
    onEndEdit();
    requestAnimationFrame(() => rowRef.current?.focus());
  };

  const summary = [
    task.title,
    done ? 'done' : null,
    `${PRIORITY_LABELS[task.priority] ?? task.priority} priority`,
    overdue ? 'overdue' : null,
  ].filter(Boolean).join(', ');

  return (
    <li className="list-none">
      <div
        ref={rowRef}
        data-task-row={task.id}
        role="group"
        tabIndex={active ? 0 : -1}
        aria-label={summary}
        aria-keyshortcuts="Enter Space F2 Delete"
        onFocus={onFocusRow}
        onKeyDown={onKeyDown}
        onClick={() => { if (!editing) openTask(task.id); }}
        className={cn(
          'group relative flex items-start gap-2.5 px-3 py-2.5 rounded-xl cursor-pointer transition-colors duration-micro',
          'hover:bg-shodh-raised focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring',
          overdue && 'shadow-[inset_3px_0_0_0_var(--c-error)]',
        )}
      >
        <button
          type="button"
          role="checkbox"
          aria-checked={done}
          aria-label={`${done ? 'Mark not done' : 'Mark done'}: ${task.title}`}
          tabIndex={inner}
          onClick={e => { e.stopPropagation(); toggle(); }}
          className={cn(
            'mt-0.5 w-4 h-4 shrink-0 rounded-full border-2 inline-flex items-center justify-center transition-colors duration-micro',
            done ? 'bg-shodh-success border-shodh-success text-shodh-ground' : 'border-shodh-border-strong hover:border-shodh-text-muted',
            FOCUS_RING,
          )}
        >
          {done && <Check className="w-2.5 h-2.5" strokeWidth={3} aria-hidden="true" />}
        </button>

        <div className="flex-1 min-w-0 flex flex-col gap-0.5">
          <div className="flex items-center gap-1.5 min-w-0">
            {editing ? (
              <input
                autoFocus
                aria-label="Task title"
                value={draft}
                onChange={e => setDraft(e.target.value)}
                onClick={e => e.stopPropagation()}
                onKeyDown={e => {
                  e.stopPropagation();
                  if (e.key === 'Enter') { e.preventDefault(); e.currentTarget.blur(); }
                  if (e.key === 'Escape') { e.preventDefault(); cancelEdit.current = true; e.currentTarget.blur(); }
                }}
                onBlur={finishEdit}
                className="flex-1 min-w-0 h-6 -my-0.5 px-1.5 -mx-1.5 rounded-md border border-shodh-border-strong bg-shodh-surface text-[13.5px] font-medium text-shodh-text focus:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              />
            ) : (
              <span
                className={cn('truncate text-[13.5px] font-medium', done ? 'line-through text-shodh-text-faint' : 'text-shodh-text')}
                title="Double-click or press F2 to rename"
                onClick={e => {
                  // Wait for a possible double-click before opening the detail sheet.
                  e.stopPropagation();
                  if (e.detail > 1) return;
                  clickTimer.current = setTimeout(() => { clickTimer.current = null; openTask(task.id); }, TITLE_CLICK_DELAY_MS);
                }}
                onDoubleClick={e => {
                  e.stopPropagation();
                  if (clickTimer.current) { clearTimeout(clickTimer.current); clickTimer.current = null; }
                  onStartEdit();
                }}
              >
                {task.title}
              </span>
            )}
            {task.source === 'agent' && (
              <Bot className="w-3.5 h-3.5 shrink-0 text-shodh-accent-text" aria-label="Created by agent" />
            )}
            {task.source === 'document' && (
              <FileText className="w-3.5 h-3.5 shrink-0 text-shodh-text-muted" aria-label="Created from a document" />
            )}
          </div>

          <div className="flex items-center gap-1 flex-wrap -ml-1.5">
            <DueMenu
              dueDate={task.dueDate}
              done={done}
              taskTitle={task.title}
              tabIndex={inner}
              onChange={dueDate => void updateTask(task.id, { dueDate })}
            />
            <ReminderBadge task={task} />
            <PriorityMenu
              priority={task.priority}
              taskTitle={task.title}
              tabIndex={inner}
              onChange={priority => void updateTask(task.id, { priority })}
            />
            {task.project && (
              <span className="h-6 px-1.5 inline-flex items-center gap-1 rounded-md text-[11.5px] text-shodh-text-muted">
                <FolderOpen className="w-3 h-3" aria-hidden="true" />
                {task.project}
              </span>
            )}
            {task.subtasks.length > 0 && (
              <span className="h-6 px-1.5 inline-flex items-center gap-1 text-[11.5px] text-shodh-text-muted tabular-nums" aria-label={`${subDone} of ${task.subtasks.length} subtasks done`}>
                <ListTodo className="w-3 h-3" aria-hidden="true" />
                {subDone}/{task.subtasks.length}
              </span>
            )}
            {task.tags.slice(0, 3).map(tag => (
              <span key={tag} className="h-5 px-1.5 inline-flex items-center rounded-md bg-shodh-raised-2 text-[11px] text-shodh-text-secondary">
                {tag}
              </span>
            ))}
            {task.tags.length > 3 && <span className="text-[11px] text-shodh-text-faint">+{task.tags.length - 3}</span>}
          </div>

          {task.description && (
            <p className="text-[12px] text-shodh-text-muted line-clamp-2 whitespace-pre-line">{task.description}</p>
          )}
        </div>

        <button
          type="button"
          tabIndex={inner}
          aria-label={`Delete ${task.title}`}
          title="Delete (you can undo)"
          onClick={e => { e.stopPropagation(); onRequestDelete(); }}
          className={cn(ICON_BUTTON, 'shrink-0 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100 hover:text-shodh-error')}
        >
          <Trash2 className="w-3.5 h-3.5" aria-hidden="true" />
        </button>
      </div>
    </li>
  );
}

// ── Mini calendar (day filter) ───────────────────────────────────

function MiniCalendar({
  activeDays,
  selected,
  onSelect,
}: {
  activeDays: Set<string>;
  selected: string | null;
  onSelect: (day: string | null) => void;
}) {
  const [view, setView] = useState(() => {
    const now = new Date();
    return { year: now.getFullYear(), month: now.getMonth() };
  });
  // Show the month of a day selected from outside the grid (e.g. by the agent).
  useEffect(() => {
    if (!selected) return;
    const [y, m] = selected.split('-').map(Number);
    setView(prev => (prev.year === y && prev.month === m - 1 ? prev : { year: y, month: m - 1 }));
  }, [selected]);
  const weeks = useMemo(() => monthGrid(view.year, view.month), [view]);
  const today = dayKey(new Date());
  const label = new Date(view.year, view.month, 1).toLocaleDateString(undefined, { month: 'long', year: 'numeric' });
  const shift = (delta: number) => {
    const d = new Date(view.year, view.month + delta, 1);
    setView({ year: d.getFullYear(), month: d.getMonth() });
  };

  return (
    <div className="p-3 rounded-xl border border-shodh-border bg-shodh-surface">
      <div className="flex items-center justify-between mb-2">
        <button type="button" onClick={() => shift(-1)} aria-label="Previous month" className={ICON_BUTTON}>
          <ChevronLeft className="w-4 h-4" aria-hidden="true" />
        </button>
        <span className="text-[12px] font-semibold text-shodh-text" aria-live="polite">{label}</span>
        <button type="button" onClick={() => shift(1)} aria-label="Next month" className={ICON_BUTTON}>
          <ChevronRight className="w-4 h-4" aria-hidden="true" />
        </button>
      </div>
      <div className="grid grid-cols-7 gap-0.5">
        {weeks[0].map(d => (
          <span key={d.key} className="text-center text-[10px] font-medium text-shodh-text-faint py-0.5" aria-hidden="true">
            {d.date.toLocaleDateString(undefined, { weekday: 'narrow' })}
          </span>
        ))}
        {weeks.flat().map(d => {
          const isSelected = d.key === selected;
          const isToday = d.key === today;
          const has = activeDays.has(d.key);
          return (
            <button
              key={d.key}
              type="button"
              aria-pressed={isSelected}
              aria-label={`${d.date.toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric' })}${has ? ', has items' : ''}`}
              onClick={() => onSelect(isSelected ? null : d.key)}
              className={cn(
                'relative h-7 rounded-md text-[11px] tabular-nums inline-flex flex-col items-center justify-center transition-colors duration-micro',
                isSelected ? 'bg-shodh-accent-soft text-shodh-accent-text font-semibold' : 'hover:bg-shodh-raised',
                !isSelected && (isToday ? 'text-shodh-accent-text font-semibold' : d.inMonth ? 'text-shodh-text-secondary' : 'text-shodh-text-faint'),
                FOCUS_RING,
              )}
            >
              {d.date.getDate()}
              {has && <span className="absolute bottom-0.5 w-1 h-1 rounded-full bg-shodh-accent-text" aria-hidden="true" />}
            </button>
          );
        })}
      </div>
    </div>
  );
}

// ── Main panel ───────────────────────────────────────────────────

/**
 * Tasks list: quick add, filters, and rows with inline title editing and
 * quick due/priority menus. Rows are one Tab stop (arrow keys move between
 * them); Enter opens the detail sheet, Space toggles done, F2 renames and
 * Delete deletes at once, with an undo window.
 */
export default function CalendarTodoPanel() {
  const { tasks, events, loading, error, refresh, deleteTask, openEvent, focusRequest } = useTasksStore();
  const [filter, setFilter] = useState<FilterTab>('all');
  const [selectedDay, setSelectedDay] = useState<string | null>(null);
  const [projectFilter, setProjectFilter] = useState<string | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const focusAfterDelete = useRef<string | null>(null);
  // Agent focus requests already applied; one made before this layout mounted is not replayed.
  const appliedFocusSeq = useRef(focusRequest?.seq ?? 0);

  const allProjects = useMemo(
    () => Array.from(new Set(tasks.map(t => t.project).filter((p): p is string => !!p))).sort(),
    [tasks],
  );

  const activeDays = useMemo(() => {
    const set = new Set<string>();
    for (const t of tasks) { const k = storedDayKey(t.dueDate); if (k) set.add(k); }
    for (const e of events) { const k = storedDayKey(e.startTime); if (k) set.add(k); }
    return set;
  }, [tasks, events]);

  const filteredTasks = useMemo(() => {
    let result = tasks;
    if (filter === 'pending') result = result.filter(t => !isDone(t));
    if (filter === 'completed') result = result.filter(isDone);
    if (selectedDay) result = result.filter(t => storedDayKey(t.dueDate) === selectedDay);
    if (projectFilter) result = result.filter(t => t.project === projectFilter);
    return sortTasks(result);
  }, [tasks, filter, selectedDay, projectFilter]);

  const upcomingEvents = useMemo(() => {
    const today = dayKey(new Date());
    return events
      .filter(e => (storedDayKey(e.startTime) ?? '') >= today)
      .sort((a, b) => (storedTime(a.startTime) ?? 0) - (storedTime(b.startTime) ?? 0))
      .slice(0, 8);
  }, [events]);

  // The roving tab stop: the active row, else the first.
  const tabStopId = filteredTasks.some(t => t.id === activeId) ? activeId : filteredTasks[0]?.id ?? null;

  const focusRow = (id: string) => {
    setActiveId(id);
    listRef.current?.querySelector<HTMLElement>(`[data-task-row="${CSS.escape(id)}"]`)?.focus();
  };

  const move = (fromId: string, to: 'prev' | 'next' | 'first' | 'last') => {
    const index = filteredTasks.findIndex(t => t.id === fromId);
    const target =
      to === 'first' ? filteredTasks[0]
        : to === 'last' ? filteredTasks[filteredTasks.length - 1]
          : filteredTasks[Math.min(filteredTasks.length - 1, Math.max(0, index + (to === 'next' ? 1 : -1)))];
    if (target) focusRow(target.id);
  };

  const doDelete = (task: TodoItem) => {
    const index = filteredTasks.findIndex(t => t.id === task.id);
    const neighbour = filteredTasks[index + 1] ?? filteredTasks[index - 1] ?? null;
    focusAfterDelete.current = neighbour?.id ?? null;
    deleteTask(task.id);
  };

  // Keep keyboard focus in the list after a row disappears.
  useEffect(() => {
    const id = focusAfterDelete.current;
    if (!id || !filteredTasks.some(t => t.id === id)) return;
    focusAfterDelete.current = null;
    focusRow(id);
  }, [filteredTasks]);

  // The agent pointed at a day or task: clear the filters that could hide it,
  // filter to its day, and bring the task's row into view (its sheet opens too).
  useEffect(() => {
    if (!focusRequest || focusRequest.seq <= appliedFocusSeq.current) return;
    appliedFocusSeq.current = focusRequest.seq;
    const task = focusRequest.taskId ? tasks.find(t => t.id === focusRequest.taskId) ?? null : null;
    setFilter('all');
    setProjectFilter(null);
    setSelectedDay(task && storedDayKey(task.dueDate) !== focusRequest.day ? null : focusRequest.day);
    if (!task) return;
    setActiveId(task.id);
    requestAnimationFrame(() => {
      listRef.current
        ?.querySelector<HTMLElement>(`[data-task-row="${CSS.escape(task.id)}"]`)
        ?.scrollIntoView({ block: 'center' });
    });
  }, [focusRequest, tasks]);

  const pendingCount = tasks.filter(t => !isDone(t)).length;
  const completedCount = tasks.length - pendingCount;
  const overdueCount = tasks.filter(t => !isDone(t) && isOverdue(t.dueDate)).length;

  if (loading && tasks.length === 0) {
    return (
      <div className="h-full flex items-center justify-center" role="status">
        <Loader2 className="w-5 h-5 animate-spin motion-reduce:animate-none text-shodh-text-muted" aria-hidden="true" />
        <span className="sr-only">Loading tasks</span>
      </div>
    );
  }

  if (error && tasks.length === 0) {
    return (
      <div className="h-full flex items-center justify-center p-6">
        <div role="alert" className="max-w-sm text-center flex flex-col items-center gap-3">
          <AlertCircle className="w-6 h-6 text-shodh-error" aria-hidden="true" />
          <p className="text-sm text-shodh-text-secondary">Could not load tasks.</p>
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

  const tabs: { id: FilterTab; label: string; count: number }[] = [
    { id: 'all', label: 'All', count: tasks.length },
    { id: 'pending', label: 'To do', count: pendingCount },
    { id: 'completed', label: 'Done', count: completedCount },
  ];

  return (
    <div className="h-full overflow-hidden flex flex-col">
      <div className="px-6 pt-3 pb-3 shrink-0">
        <h1 className="text-lg font-bold text-shodh-text">Tasks</h1>
        <p className="text-[12px] text-shodh-text-muted mt-0.5">
          {pendingCount} to do
          {overdueCount > 0 && <span className="text-shodh-error"> · {overdueCount} overdue</span>}
          {completedCount > 0 && ` · ${completedCount} done`}
        </p>
      </div>

      <div className="flex-1 min-h-0 flex gap-5 px-6 pb-6">
        <section aria-label="Task list" className="flex-1 min-w-0 flex flex-col min-h-0 gap-3">
          <QuickAdd defaultDay={selectedDay} />

          <div className="flex items-center gap-1 shrink-0 flex-wrap">
            {tabs.map(tab => (
              <button
                key={tab.id}
                type="button"
                aria-pressed={filter === tab.id}
                onClick={() => setFilter(tab.id)}
                className={cn(
                  'h-7 px-3 rounded-full text-[12px] font-medium transition-colors duration-micro',
                  filter === tab.id ? 'bg-shodh-accent-soft text-shodh-accent-text' : 'text-shodh-text-muted hover:text-shodh-text hover:bg-shodh-raised',
                  FOCUS_RING,
                )}
              >
                {tab.label}
                <span className="ml-1 tabular-nums opacity-70">{tab.count}</span>
              </button>
            ))}
            <div className="ml-auto flex items-center gap-1">
              {projectFilter && (
                <button
                  type="button"
                  onClick={() => setProjectFilter(null)}
                  aria-label={`Clear project filter ${projectFilter}`}
                  className={cn('h-7 px-2 rounded-full inline-flex items-center gap-1 text-[11.5px] bg-shodh-raised text-shodh-text-secondary hover:text-shodh-text', FOCUS_RING)}
                >
                  <FolderOpen className="w-3 h-3" aria-hidden="true" />
                  {projectFilter}
                  <X className="w-3 h-3" aria-hidden="true" />
                </button>
              )}
              {selectedDay && (
                <button
                  type="button"
                  onClick={() => setSelectedDay(null)}
                  aria-label="Clear day filter"
                  className={cn('h-7 px-2 rounded-full inline-flex items-center gap-1 text-[11.5px] bg-shodh-raised text-shodh-text-secondary hover:text-shodh-text', FOCUS_RING)}
                >
                  {new Date(`${selectedDay}T00:00`).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}
                  <X className="w-3 h-3" aria-hidden="true" />
                </button>
              )}
            </div>
          </div>

          <div className="flex-1 min-h-0 overflow-y-auto scrollbar-thin -mx-1 px-1">
            {filteredTasks.length === 0 ? (
              <div className="flex flex-col items-center justify-center py-12 text-center gap-1">
                <CheckCircle2 className="w-9 h-9 mb-2 text-shodh-text-faint opacity-50" aria-hidden="true" />
                <p className="text-[13px] font-medium text-shodh-text-secondary">
                  {filter === 'completed' ? 'No completed tasks' : selectedDay ? 'Nothing due on this day' : 'No tasks yet'}
                </p>
                <p className="text-[12px] text-shodh-text-faint">
                  {filter === 'all' && !selectedDay && !projectFilter
                    ? 'Add one above, or ask Shodh to create tasks for you.'
                    : 'Try a different filter or day.'}
                </p>
              </div>
            ) : (
              <ul ref={listRef} aria-label="Tasks" className="flex flex-col gap-0.5">
                {filteredTasks.map(task => (
                  <TaskRow
                    key={task.id}
                    task={task}
                    active={task.id === tabStopId}
                    editing={task.id === editingId}
                    onFocusRow={() => setActiveId(task.id)}
                    onStartEdit={() => { setActiveId(task.id); setEditingId(task.id); }}
                    onEndEdit={() => setEditingId(null)}
                    onRequestDelete={() => doDelete(task)}
                    onMove={to => move(task.id, to)}
                  />
                ))}
              </ul>
            )}
          </div>
        </section>

        <aside aria-label="Calendar and projects" className="w-64 shrink-0 flex flex-col gap-5 min-h-0 overflow-y-auto scrollbar-thin">
          <MiniCalendar activeDays={activeDays} selected={selectedDay} onSelect={setSelectedDay} />

          {allProjects.length > 0 && (
            <div className="flex flex-col gap-1">
              <h2 className="text-[11px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint mb-1">Projects</h2>
              {allProjects.map(proj => {
                const count = tasks.filter(t => t.project === proj && !isDone(t)).length;
                const isActive = projectFilter === proj;
                return (
                  <button
                    key={proj}
                    type="button"
                    aria-pressed={isActive}
                    onClick={() => setProjectFilter(isActive ? null : proj)}
                    className={cn(
                      'h-8 px-2.5 rounded-lg flex items-center gap-2 text-left text-[12.5px] transition-colors duration-micro',
                      isActive ? 'bg-shodh-accent-soft text-shodh-accent-text' : 'text-shodh-text-secondary hover:bg-shodh-raised',
                      FOCUS_RING,
                    )}
                  >
                    <FolderOpen className="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
                    <span className="flex-1 truncate">{proj}</span>
                    <span className="tabular-nums opacity-70">{count}</span>
                  </button>
                );
              })}
            </div>
          )}

          <div className="flex flex-col gap-1.5">
            <h2 className="text-[11px] font-semibold uppercase tracking-[0.08em] text-shodh-text-faint mb-0.5">Upcoming events</h2>
            {upcomingEvents.length === 0 ? (
              <p className="text-[12px] text-shodh-text-faint">No upcoming events.</p>
            ) : (
              <ul className="flex flex-col gap-1.5">
                {upcomingEvents.map(ev => (
                  <li key={ev.id}>
                    <button
                      type="button"
                      onClick={() => openEvent(ev.id)}
                      className={cn('w-full flex items-start gap-2 px-3 py-2 rounded-lg border border-shodh-border bg-shodh-surface text-left hover:bg-shodh-raised transition-colors duration-micro', FOCUS_RING)}
                    >
                      <span className="mt-1.5 w-1.5 h-1.5 rounded-full bg-shodh-info shrink-0" aria-hidden="true" />
                      <span className="flex-1 min-w-0 flex flex-col">
                        <span className="text-[12.5px] text-shodh-text truncate">{ev.title}</span>
                        <span className="text-[11.5px] text-shodh-text-faint truncate">
                          {formatEventWhen(ev)}{ev.location ? ` · ${ev.location}` : ''}
                        </span>
                      </span>
                      {ev.source === 'agent' && <Bot className="w-3.5 h-3.5 mt-0.5 shrink-0 text-shodh-text-muted" aria-label="Created by agent" />}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>

          <dl className="p-3 rounded-xl border border-shodh-border bg-shodh-surface grid grid-cols-[1fr_auto] gap-y-1.5 text-[12px]">
            <dt className="text-shodh-text-muted">Total tasks</dt><dd className="tabular-nums font-semibold text-shodh-text text-right">{tasks.length}</dd>
            <dt className="text-shodh-text-muted">To do</dt><dd className="tabular-nums font-semibold text-shodh-warning text-right">{pendingCount}</dd>
            <dt className="text-shodh-text-muted">Overdue</dt><dd className="tabular-nums font-semibold text-shodh-error text-right">{overdueCount}</dd>
            <dt className="text-shodh-text-muted">Done</dt><dd className="tabular-nums font-semibold text-shodh-success text-right">{completedCount}</dd>
            <dt className="text-shodh-text-muted">Events</dt><dd className="tabular-nums font-semibold text-shodh-info text-right">{events.length}</dd>
          </dl>
        </aside>
      </div>

    </div>
  );
}
