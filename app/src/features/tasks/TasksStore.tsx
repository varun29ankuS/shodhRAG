import React, { createContext, useCallback, useContext, useEffect, useMemo, useReducer, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { notify } from '../../lib/notify';
import { sameMoment } from './dueDate';
import { sameTags } from './tags';
import {
  changedFields,
  initialStore,
  isTempSubtaskId,
  storeReducer,
  TEMP_SUBTASK_PREFIX,
  visibleEvents,
  visibleTasks,
} from './taskStore';
import type { PendingOp } from './taskStore';
import type { CalendarEvent, TodoItem } from './types';

/** How long a deleted task or event can be restored from the toast. */
export const UNDO_WINDOW_MS = 6000;

/** Task fields `update_task` can set. It cannot clear a field (None keeps it). */
export type TaskPatch = Partial<Pick<TodoItem, 'title' | 'description' | 'dueDate' | 'priority' | 'status' | 'tags' | 'project'>>;

/** Event fields `update_event` can set. It cannot clear `endTime`. */
export type EventPatch = Partial<Pick<CalendarEvent, 'title' | 'description' | 'startTime' | 'endTime' | 'allDay'>>;

export interface NewTaskInput {
  title: string;
  dueDate?: string | null;
  priority?: string;
  project?: string | null;
}

type Detail = { kind: 'task'; id: string } | { kind: 'event'; id: string } | null;

interface TasksStoreValue {
  tasks: TodoItem[];
  events: CalendarEvent[];
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  createTask: (input: NewTaskInput) => Promise<TodoItem | null>;
  /** Resolves true when saved (or nothing changed), false when rolled back. */
  updateTask: (id: string, patch: TaskPatch) => Promise<boolean>;
  /** Hides the task at once; it is deleted when the undo window ends. */
  deleteTask: (id: string) => void;
  addSubtask: (taskId: string, title: string) => Promise<boolean>;
  toggleSubtask: (taskId: string, subtaskId: string) => Promise<boolean>;
  deleteSubtask: (taskId: string, subtaskId: string) => Promise<boolean>;
  updateEvent: (id: string, patch: EventPatch) => Promise<boolean>;
  deleteEvent: (id: string) => void;
  openTask: (id: string) => void;
  openEvent: (id: string) => void;
  /** The task or event whose detail sheet is open (rendered by `TasksView`). */
  detailTask: TodoItem | null;
  detailEvent: CalendarEvent | null;
  closeDetail: () => void;
}

const TasksStoreContext = createContext<TasksStoreValue | null>(null);

export function useTasksStore(): TasksStoreValue {
  const value = useContext(TasksStoreContext);
  if (!value) throw new Error('useTasksStore must be used inside <TasksStoreProvider>');
  return value;
}

function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Wire args for `update_task`: camelCase keys (Tauri maps them to snake_case), changed fields only, never `reminder`. */
function taskArgs(id: string, patch: TaskPatch): Record<string, unknown> {
  const args: Record<string, unknown> = { id };
  for (const key of ['title', 'description', 'dueDate', 'priority', 'status', 'tags', 'project'] as const) {
    if (patch[key] !== undefined) args[key] = patch[key];
  }
  return args;
}

function eventArgs(id: string, patch: EventPatch): Record<string, unknown> {
  const args: Record<string, unknown> = { id };
  for (const key of ['title', 'description', 'startTime', 'endTime', 'allDay'] as const) {
    if (patch[key] !== undefined) args[key] = patch[key];
  }
  return args;
}

interface PendingDelete {
  kind: 'task' | 'event';
  id: string;
  timer: ReturnType<typeof setTimeout>;
  toastId: string | number;
}

/**
 * One store for both Tasks layouts: a single fetch, the `calendar-changed`
 * listener, optimistic mutations with rollback, delete-with-undo, and which
 * detail sheet is open. Mounted above the List/Calendar switch so a
 * pending undo survives switching layouts.
 */
export function TasksStoreProvider({ children }: { children: React.ReactNode }) {
  const [state, dispatch] = useReducer(storeReducer, initialStore);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [detail, setDetail] = useState<Detail>(null);
  const opSeq = useRef(0);
  const pendingDeletes = useRef(new Map<number, PendingDelete>());

  const tasks = useMemo(() => visibleTasks(state), [state]);
  const events = useMemo(() => visibleEvents(state), [state]);

  // Latest visible records for callbacks, without re-creating them on every change.
  const latest = useRef({ tasks, events });
  latest.current = { tasks, events };

  const refresh = useCallback(async () => {
    try {
      const [t, e] = await Promise.all([
        invoke<TodoItem[]>('load_tasks'),
        invoke<CalendarEvent[]>('load_events'),
      ]);
      dispatch({ type: 'snapshot', tasks: t, events: e });
      setError(null);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Every task/event write (ours, the agent's, subtasks) emits this.
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;
    listen('calendar-changed', () => { void refresh(); })
      .then(fn => { if (active) unlisten = fn; else fn(); })
      .catch(err => console.error('Failed to listen for calendar changes:', err));
    return () => { active = false; unlisten?.(); };
  }, [refresh]);

  const nextOp = () => ++opSeq.current;

  /** Apply `patch` optimistically, run `command`, then commit or roll back. */
  const runTaskOp = useCallback(async (
    id: string,
    patch: Partial<TodoItem>,
    command: () => Promise<TodoItem>,
    failure: string,
  ): Promise<boolean> => {
    const opId = nextOp();
    dispatch({ type: 'begin', op: { opId, kind: 'task', id, patch } });
    try {
      const task = await command();
      dispatch({ type: 'commit', opId, task });
      return true;
    } catch (err) {
      dispatch({ type: 'rollback', opId });
      notify.error(failure, { description: errorText(err) });
      return false;
    }
  }, []);

  const updateTask = useCallback(async (id: string, patch: TaskPatch): Promise<boolean> => {
    const current = latest.current.tasks.find(t => t.id === id);
    if (!current) return false;
    const requested: TaskPatch = { ...patch };
    // update_task treats a missing value as "keep": these cannot be cleared.
    if (requested.dueDate !== undefined && !requested.dueDate) delete requested.dueDate;
    if (requested.project !== undefined && !requested.project?.trim()) delete requested.project;
    if (requested.title !== undefined && !requested.title.trim()) delete requested.title;
    const changes = changedFields(current, requested);
    if (changes.dueDate !== undefined && sameMoment(changes.dueDate, current.dueDate)) delete changes.dueDate;
    if (changes.tags !== undefined && sameTags(changes.tags, current.tags)) delete changes.tags;
    if (Object.keys(changes).length === 0) return true;
    return runTaskOp(
      id,
      changes,
      () => invoke<TodoItem>('update_task', taskArgs(id, changes)),
      'Could not save the task',
    );
  }, [runTaskOp]);

  const createTask = useCallback(async (input: NewTaskInput): Promise<TodoItem | null> => {
    const args: Record<string, unknown> = { title: input.title.trim(), source: 'user' };
    if (input.dueDate) args.dueDate = input.dueDate;
    if (input.priority) args.priority = input.priority;
    if (input.project?.trim()) args.project = input.project.trim();
    try {
      const task = await invoke<TodoItem>('create_task', args);
      dispatch({ type: 'insertTask', task });
      return task;
    } catch (err) {
      notify.error('Could not add the task', { description: errorText(err) });
      return null;
    }
  }, []);

  const addSubtask = useCallback(async (taskId: string, title: string): Promise<boolean> => {
    const current = latest.current.tasks.find(t => t.id === taskId);
    const text = title.trim();
    if (!current || !text) return false;
    const temp = { id: `${TEMP_SUBTASK_PREFIX}${nextOp()}`, title: text, completed: false };
    return runTaskOp(
      taskId,
      { subtasks: [...current.subtasks, temp] },
      () => invoke<TodoItem>('add_subtask', { taskId, title: text }),
      'Could not add the subtask',
    );
  }, [runTaskOp]);

  const toggleSubtask = useCallback(async (taskId: string, subtaskId: string): Promise<boolean> => {
    const current = latest.current.tasks.find(t => t.id === taskId);
    if (!current || isTempSubtaskId(subtaskId)) return false;
    return runTaskOp(
      taskId,
      { subtasks: current.subtasks.map(s => (s.id === subtaskId ? { ...s, completed: !s.completed } : s)) },
      () => invoke<TodoItem>('toggle_subtask', { taskId, subtaskId }),
      'Could not update the subtask',
    );
  }, [runTaskOp]);

  const deleteSubtask = useCallback(async (taskId: string, subtaskId: string): Promise<boolean> => {
    const current = latest.current.tasks.find(t => t.id === taskId);
    if (!current || isTempSubtaskId(subtaskId)) return false;
    return runTaskOp(
      taskId,
      { subtasks: current.subtasks.filter(s => s.id !== subtaskId) },
      () => invoke<TodoItem>('delete_subtask', { taskId, subtaskId }),
      'Could not delete the subtask',
    );
  }, [runTaskOp]);

  const updateEvent = useCallback(async (id: string, patch: EventPatch): Promise<boolean> => {
    const current = latest.current.events.find(e => e.id === id);
    if (!current) return false;
    const requested: EventPatch = { ...patch };
    if (requested.endTime !== undefined && !requested.endTime) delete requested.endTime;
    if (requested.startTime !== undefined && !requested.startTime) delete requested.startTime;
    if (requested.title !== undefined && !requested.title.trim()) delete requested.title;
    const changes = changedFields(current, requested);
    if (changes.startTime !== undefined && sameMoment(changes.startTime, current.startTime)) delete changes.startTime;
    if (changes.endTime !== undefined && sameMoment(changes.endTime, current.endTime)) delete changes.endTime;
    if (Object.keys(changes).length === 0) return true;
    const opId = nextOp();
    dispatch({ type: 'begin', op: { opId, kind: 'event', id, patch: changes } });
    try {
      const event = await invoke<CalendarEvent>('update_event', eventArgs(id, changes));
      dispatch({ type: 'commit', opId, event });
      return true;
    } catch (err) {
      dispatch({ type: 'rollback', opId });
      notify.error('Could not save the event', { description: errorText(err) });
      return false;
    }
  }, []);

  // ── Delete with undo ────────────────────────────────────────────
  //
  // The record is hidden at once and deleted only when the undo window ends
  // (or the toast is closed). Undo just drops the pending op, so nothing has
  // to be recreated: id, subtasks, status, completion time and provenance are
  // never lost. If the app quits inside the window the record survives.

  const commitDelete = useCallback(async (opId: number) => {
    const entry = pendingDeletes.current.get(opId);
    if (!entry) return;
    pendingDeletes.current.delete(opId);
    clearTimeout(entry.timer);
    toast.dismiss(entry.toastId);
    const command = entry.kind === 'task' ? 'delete_task' : 'delete_event';
    try {
      await invoke(command, { id: entry.id });
      dispatch({ type: 'commit', opId, removed: true });
    } catch (err) {
      const message = errorText(err);
      if (/not found/i.test(message)) {
        // Already gone (deleted elsewhere): the outcome the user asked for.
        dispatch({ type: 'commit', opId, removed: true });
        return;
      }
      dispatch({ type: 'rollback', opId });
      notify.error(entry.kind === 'task' ? 'Could not delete the task' : 'Could not delete the event', { description: message });
    }
  }, []);

  const undoDelete = useCallback((opId: number) => {
    const entry = pendingDeletes.current.get(opId);
    if (!entry) return;
    pendingDeletes.current.delete(opId);
    clearTimeout(entry.timer);
    toast.dismiss(entry.toastId);
    dispatch({ type: 'rollback', opId });
  }, []);

  const scheduleDelete = useCallback((kind: 'task' | 'event', id: string, title: string) => {
    const opId = nextOp();
    const op: PendingOp = kind === 'task' ? { opId, kind, id, remove: true } : { opId, kind, id, remove: true };
    dispatch({ type: 'begin', op });
    setDetail(prev => (prev && prev.kind === kind && prev.id === id ? null : prev));
    const toastId = toast(kind === 'task' ? 'Task deleted' : 'Event deleted', {
      description: title,
      // Dismissed by commitDelete when the window ends, so Undo is never offered after the delete ran.
      duration: Infinity,
      action: { label: 'Undo', onClick: () => undoDelete(opId) },
      onDismiss: () => { void commitDelete(opId); },
    });
    const timer = setTimeout(() => { void commitDelete(opId); }, UNDO_WINDOW_MS);
    pendingDeletes.current.set(opId, { kind, id, timer, toastId });
  }, [commitDelete, undoDelete]);

  const deleteTask = useCallback((id: string) => {
    const task = latest.current.tasks.find(t => t.id === id);
    if (task) scheduleDelete('task', id, task.title);
  }, [scheduleDelete]);

  const deleteEvent = useCallback((id: string) => {
    const event = latest.current.events.find(e => e.id === id);
    if (event) scheduleDelete('event', id, event.title);
  }, [scheduleDelete]);

  // Leaving Tasks inside the window carries out the deletes the user asked for.
  useEffect(() => {
    const deletes = pendingDeletes.current;
    return () => {
      for (const opId of [...deletes.keys()]) void commitDelete(opId);
    };
  }, [commitDelete]);

  const openTask = useCallback((id: string) => setDetail({ kind: 'task', id }), []);
  const openEvent = useCallback((id: string) => setDetail({ kind: 'event', id }), []);
  const closeDetail = useCallback(() => setDetail(null), []);

  const detailTask = detail?.kind === 'task' ? tasks.find(t => t.id === detail.id) ?? null : null;
  const detailEvent = detail?.kind === 'event' ? events.find(e => e.id === detail.id) ?? null : null;

  // The record went away (deleted elsewhere): close its sheet.
  useEffect(() => {
    if (loading || !detail) return;
    if (detail.kind === 'task' ? !detailTask : !detailEvent) setDetail(null);
  }, [loading, detail, detailTask, detailEvent]);

  const value = useMemo<TasksStoreValue>(() => ({
    tasks,
    events,
    loading,
    error,
    refresh,
    createTask,
    updateTask,
    deleteTask,
    addSubtask,
    toggleSubtask,
    deleteSubtask,
    updateEvent,
    deleteEvent,
    openTask,
    openEvent,
    detailTask,
    detailEvent,
    closeDetail,
  }), [tasks, events, loading, error, refresh, createTask, updateTask, deleteTask, addSubtask, toggleSubtask, deleteSubtask, updateEvent, deleteEvent, openTask, openEvent, detailTask, detailEvent, closeDetail]);

  return <TasksStoreContext.Provider value={value}>{children}</TasksStoreContext.Provider>;
}
