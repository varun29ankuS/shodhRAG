import React, { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import { AlertCircle, Clock, Flag, ListTodo, Loader2, Plus } from 'lucide-react';
import { cn } from '../../lib/utils';
import { freshness } from '../../lib/freshness';
import { StaleNotice } from '../../components/ui/StaleNotice';
import { adjacentStatus, boardColumns, columnOf } from './board';
import type { BoardColumn } from './board';
import { dueLabel, isOverdue } from './dueDate';
import { FOCUS_RING } from './fields';
import { PRIORITY_TEXT, StatusMenu } from './QuickMenus';
import { TasksStoreProvider, useTasksStore } from './TasksStore';
import TaskDetailSheet from './TaskDetailSheet';
import { isDone, PRIORITY_LABELS, STATUS_LABELS } from './types';
import type { TodoItem } from './types';

/** How far (px) a pointer moves before a press on a card becomes a drag. */
const DRAG_THRESHOLD_PX = 6;

/** Controls inside a card that keep their own pointer handling; the title (`data-card-handle`) also drags. */
const CARD_CONTROLS = 'button, a, input, select, textarea, [role="menu"]';

interface DragState {
  id: string;
  from: string;
  pointerId: number;
  startX: number;
  startY: number;
  dx: number;
  dy: number;
  /** Past the threshold: the card follows the pointer. */
  active: boolean;
  /** The column under the pointer. */
  over: string | null;
}

/** The status of the board column under a point, ignoring the dragged card itself. */
function columnAt(x: number, y: number, dragged: HTMLElement | null): string | null {
  for (const el of document.elementsFromPoint(x, y)) {
    if (dragged && dragged.contains(el)) continue;
    const column = el.closest<HTMLElement>('[data-board-column]');
    if (column) return column.dataset.boardColumn ?? null;
  }
  return null;
}

function AddTask({ workspaceId, workspaceName }: { workspaceId: string; workspaceName: string }) {
  const { createTask } = useTasksStore();
  const [title, setTitle] = useState('');
  const [busy, setBusy] = useState(false);
  const inputId = useId();
  const inputRef = useRef<HTMLInputElement>(null);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const text = title.trim();
    if (!text || busy) return;
    setBusy(true);
    const created = await createTask({ title: text, workspaceId });
    setBusy(false);
    if (created) setTitle('');
    inputRef.current?.focus();
  };

  return (
    <form onSubmit={submit} aria-label={`Add a task to ${workspaceName}`} className="flex items-center gap-2 p-1.5 pl-3 rounded-xl border border-shodh-border bg-shodh-raised">
      <Plus className="w-4 h-4 shrink-0 text-shodh-text-faint" aria-hidden="true" />
      <label htmlFor={inputId} className="sr-only">New task in {workspaceName}</label>
      <input
        ref={inputRef}
        id={inputId}
        type="text"
        value={title}
        onChange={e => setTitle(e.target.value)}
        onKeyDown={e => { if (e.key === 'Escape' && title) { e.preventDefault(); setTitle(''); } }}
        placeholder="Add a task to this workspace"
        className="flex-1 min-w-0 h-8 bg-transparent text-small text-shodh-text placeholder:text-shodh-text-faint focus:outline-none"
      />
      <button
        type="submit"
        disabled={!title.trim() || busy}
        className={cn('h-8 px-3 rounded-lg bg-shodh-accent text-shodh-on-accent text-small font-medium hover:bg-shodh-accent-hover disabled:opacity-50 transition-colors duration-micro inline-flex items-center gap-1.5', FOCUS_RING)}
      >
        {busy && <Loader2 className="w-3.5 h-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />}
        Add
      </button>
    </form>
  );
}

function Card({
  task,
  drag,
  onPointerDown,
  onMove,
  wasDragged,
}: {
  task: TodoItem;
  drag: DragState | null;
  onPointerDown: (e: React.PointerEvent<HTMLLIElement>, task: TodoItem) => void;
  onMove: (task: TodoItem, status: string) => void;
  /** Whether the last press on this card ended a drag (its click must not open the card). */
  wasDragged: () => boolean;
}) {
  const { openTask } = useTasksStore();
  const done = isDone(task);
  const overdue = !done && isOverdue(task.dueDate);
  const subDone = task.subtasks.filter(s => s.completed).length;
  const dragging = drag?.active === true && drag.id === task.id;
  const status = columnOf(task);

  const onKeyDown = (e: React.KeyboardEvent<HTMLButtonElement>) => {
    if (!e.altKey || (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight')) return;
    const next = adjacentStatus(status, e.key === 'ArrowRight' ? 1 : -1);
    e.preventDefault();
    if (next) onMove(task, next);
  };

  return (
    <li
      data-board-card={task.id}
      onPointerDown={e => onPointerDown(e, task)}
      style={dragging && drag ? { transform: `translate(${drag.dx}px, ${drag.dy}px)` } : undefined}
      className={cn(
        'list-none rounded-xl border border-shodh-border bg-shodh-surface px-3 py-2.5 flex flex-col gap-1.5 select-none touch-none cursor-grab',
        'transition-shadow duration-micro hover:border-shodh-border-strong',
        overdue && 'shadow-[inset_3px_0_0_0_var(--c-error)]',
        dragging && 'relative z-30 cursor-grabbing shadow-[0_12px_36px_rgba(0,0,0,0.32)] opacity-90',
      )}
    >
      <button
        type="button"
        data-card-handle
        onClick={() => { if (!wasDragged()) openTask(task.id); }}
        onKeyDown={onKeyDown}
        aria-keyshortcuts="Alt+ArrowLeft Alt+ArrowRight"
        aria-label={`${task.title}. ${STATUS_LABELS[status] ?? status}${overdue ? ', overdue' : ''}. Open the task; Alt+Arrow keys move it between columns.`}
        className={cn(
          'text-left text-small font-medium rounded-md -mx-1 px-1 break-words',
          done ? 'line-through text-shodh-text-faint' : 'text-shodh-text hover:underline',
          FOCUS_RING,
        )}
      >
        {task.title}
      </button>
      <div className="flex items-center gap-1 flex-wrap -ml-1.5">
        <StatusMenu
          status={status}
          taskTitle={task.title}
          tabIndex={0}
          onChange={next => onMove(task, next)}
        />
        {task.dueDate && (
          <span className={cn('h-6 px-1.5 inline-flex items-center gap-1 text-caption', overdue ? 'text-shodh-error' : 'text-shodh-text-muted')}>
            <Clock className="w-3 h-3" aria-hidden="true" />
            {dueLabel(task.dueDate)}
          </span>
        )}
        <span
          className={cn('h-6 px-1 inline-flex items-center', PRIORITY_TEXT[task.priority] ?? 'text-shodh-text-muted')}
          title={`${PRIORITY_LABELS[task.priority] ?? task.priority} priority`}
        >
          <Flag className="w-3 h-3" aria-hidden="true" />
          <span className="sr-only">{PRIORITY_LABELS[task.priority] ?? task.priority} priority</span>
        </span>
        {task.subtasks.length > 0 && (
          <span className="h-6 px-1.5 inline-flex items-center gap-1 text-caption text-shodh-text-muted tabular-nums" aria-label={`${subDone} of ${task.subtasks.length} subtasks done`}>
            <ListTodo className="w-3 h-3" aria-hidden="true" />
            {subDone}/{task.subtasks.length}
          </span>
        )}
      </div>
    </li>
  );
}

function Column({
  column,
  drag,
  children,
}: {
  column: BoardColumn;
  drag: DragState | null;
  children: React.ReactNode;
}) {
  const headingId = useId();
  const target = drag?.active === true && drag.over === column.status && drag.from !== column.status;
  return (
    <section
      aria-labelledby={headingId}
      data-board-column={column.status}
      className={cn(
        'min-w-0 flex flex-col gap-2 rounded-xl border p-2 transition-colors duration-micro',
        target ? 'border-shodh-accent bg-shodh-raised' : 'border-shodh-border-subtle bg-shodh-raised/40',
      )}
    >
      <h3 id={headingId} className="m-0 px-1 pt-0.5 text-caption font-semibold text-shodh-text-secondary">
        {column.label} <span className="font-normal text-shodh-text-muted tabular-nums">({column.tasks.length})</span>
      </h3>
      {column.tasks.length === 0 ? (
        <p className="m-0 px-1 py-4 text-center text-caption text-shodh-text-faint rounded-lg border border-dashed border-shodh-border-subtle">
          No tasks
        </p>
      ) : (
        <ul className="m-0 p-0 flex flex-col gap-2">{children}</ul>
      )}
    </section>
  );
}

/**
 * A workspace's tasks as a board, one column per status. A card moves by
 * dragging it to another column, by its status menu, or with Alt+Arrow keys
 * on its title; clicking the title opens the task editor. Dragging uses
 * pointer events rather than HTML5 drag and drop, which the desktop
 * webview's file-drop handling blocks.
 */
function TaskBoard({ workspaceId, workspaceName }: { workspaceId: string; workspaceName: string }) {
  const { tasks, loading, error, refresh, updateTask } = useTasksStore();
  const columns = useMemo(() => boardColumns(tasks, workspaceId), [tasks, workspaceId]);
  const [drag, setDrag] = useState<DragState | null>(null);
  const [announcement, setAnnouncement] = useState('');
  const dragRef = useRef<DragState | null>(null);
  const cardRef = useRef<HTMLElement | null>(null);
  const draggedId = useRef<string | null>(null);

  const setDragState = useCallback((next: DragState | null) => {
    dragRef.current = next;
    setDrag(next);
  }, []);

  // The click that ends a drag must not open the card. It is dispatched right
  // after the pointer is released, so the mark lasts until the next task only.
  const suppressClick = useCallback((id: string) => {
    draggedId.current = id;
    setTimeout(() => { draggedId.current = null; }, 0);
  }, []);

  const move = useCallback((task: TodoItem, status: string) => {
    if (columnOf(task) === status) return;
    setAnnouncement(`Moved “${task.title}” to ${STATUS_LABELS[status] ?? status}`);
    void updateTask(task.id, { status });
  }, [updateTask]);

  const onPointerDown = useCallback((e: React.PointerEvent<HTMLLIElement>, task: TodoItem) => {
    if (e.button !== 0 || !e.isPrimary) return;
    const control = (e.target as Element).closest(CARD_CONTROLS);
    if (control && !control.hasAttribute('data-card-handle')) return;
    draggedId.current = null;
    cardRef.current = e.currentTarget;
    setDragState({
      id: task.id,
      from: columnOf(task),
      pointerId: e.pointerId,
      startX: e.clientX,
      startY: e.clientY,
      dx: 0,
      dy: 0,
      active: false,
      over: null,
    });
  }, [setDragState]);

  // Latest values for the window listeners below, which live as long as a press.
  const latest = useRef({ tasks, move });
  latest.current = { tasks, move };
  const pressed = drag !== null;

  // Follow the pointer on the window while a press is down, so a fast drag
  // that leaves the card is not lost.
  useEffect(() => {
    if (!pressed) return;
    const onMovePointer = (e: PointerEvent) => {
      const current = dragRef.current;
      if (!current || e.pointerId !== current.pointerId) return;
      const dx = e.clientX - current.startX;
      const dy = e.clientY - current.startY;
      const active = current.active || Math.hypot(dx, dy) >= DRAG_THRESHOLD_PX;
      if (!active) return;
      e.preventDefault();
      setDragState({ ...current, dx, dy, active, over: columnAt(e.clientX, e.clientY, cardRef.current) });
    };
    const finish = (e: PointerEvent, drop: boolean) => {
      const current = dragRef.current;
      if (!current || e.pointerId !== current.pointerId) return;
      setDragState(null);
      if (!current.active) return;
      suppressClick(current.id);
      const task = latest.current.tasks.find(t => t.id === current.id);
      if (drop && task && current.over && current.over !== current.from) latest.current.move(task, current.over);
    };
    const onUp = (e: PointerEvent) => finish(e, true);
    const onCancel = (e: PointerEvent) => finish(e, false);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && dragRef.current?.active) {
        e.preventDefault();
        suppressClick(dragRef.current.id);
        setDragState(null);
      }
    };
    window.addEventListener('pointermove', onMovePointer, { passive: false });
    window.addEventListener('pointerup', onUp);
    window.addEventListener('pointercancel', onCancel);
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('pointermove', onMovePointer);
      window.removeEventListener('pointerup', onUp);
      window.removeEventListener('pointercancel', onCancel);
      window.removeEventListener('keydown', onKey);
    };
  }, [pressed, setDragState, suppressClick]);

  const wasDragged = useCallback(() => {
    const dragged = draggedId.current !== null;
    draggedId.current = null;
    return dragged;
  }, []);

  if (loading && tasks.length === 0) {
    return (
      <div className="py-10 flex items-center justify-center" role="status">
        <Loader2 className="w-5 h-5 animate-spin motion-reduce:animate-none text-shodh-text-muted" aria-hidden="true" />
        <span className="sr-only">Loading tasks</span>
      </div>
    );
  }

  if (error && tasks.length === 0) {
    return (
      <div role="alert" className="py-8 flex flex-col items-center gap-3 text-center">
        <AlertCircle className="w-6 h-6 text-shodh-error" aria-hidden="true" />
        <p className="m-0 text-sm text-shodh-text-secondary">Could not load tasks.</p>
        <p className="m-0 text-caption text-shodh-text-faint break-words">{error}</p>
        <button
          type="button"
          onClick={() => void refresh()}
          className={cn('h-8 px-3 inline-flex items-center gap-1.5 rounded-lg border border-shodh-border text-small hover:bg-shodh-raised', FOCUS_RING)}
        >
          Try again
        </button>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-3">
      {/* Over tasks kept from an earlier read, a failed refresh is said (lib/freshness). */}
      {freshness(tasks.length > 0, error) === 'stale' && <StaleNotice error={error} onRetry={() => void refresh()} />}
      <AddTask workspaceId={workspaceId} workspaceName={workspaceName} />
      <div className="grid grid-cols-1 md:grid-cols-3 gap-3 items-start">
        {columns.map(column => (
          <Column key={column.status} column={column} drag={drag}>
            {column.tasks.map(task => (
              <Card key={task.id} task={task} drag={drag} onPointerDown={onPointerDown} onMove={move} wasDragged={wasDragged} />
            ))}
          </Column>
        ))}
      </div>
      <p className="sr-only" aria-live="polite">{announcement}</p>
    </div>
  );
}

/**
 * The Tasks tab of a workspace: its board, with the task editor. It has its
 * own task store, which leaves the agent's `show_calendar` requests to the
 * Tasks view.
 */
export default function WorkspaceTaskBoard({ workspaceId, workspaceName }: { workspaceId: string; workspaceName: string }) {
  return (
    <TasksStoreProvider followAgent={false}>
      <TaskBoard workspaceId={workspaceId} workspaceName={workspaceName} />
      <BoardTaskSheet />
    </TasksStoreProvider>
  );
}

function BoardTaskSheet() {
  const { detailTask, closeDetail } = useTasksStore();
  return <TaskDetailSheet task={detailTask} onClose={closeDetail} />;
}
