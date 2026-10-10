import { UNDO_WINDOW_MS } from '../../lib/undoQueue';
import React, { createContext, useCallback, useContext, useEffect, useMemo, useReducer, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { notify } from '../../lib/notify';
import { useNavigationTarget } from '../agent/useNavigationTarget';
import { sameMoment, storedDayKey } from './dueDate';
import { sameTags } from './tags';
import {
  changedFields,
  eventArgs,
  initialStore,
  isTempSubtaskId,
  storeReducer,
  taskArgs,
  TEMP_SUBTASK_PREFIX,
  visibleEvents,
  visibleTasks,
} from './taskStore';
import type { EventPatch, PendingOp, TaskPatch } from './taskStore';
import type { CalendarEvent, TodoItem } from './types';

export type { EventPatch, TaskPatch } from './taskStore';

/** How long a deleted task or event can be restored from the toast (the app's undo window). */
export { UNDO_WINDOW_MS };

export interface NewTaskInput {
  title: string;
  dueDate?: string | null;
  priority?: string;
  project?: string | null;
}

export interface NewEventInput {
  title: string;
  /** Stored moment: a day key for all-day events, else a local date-time. */
  startTime: string;
  endTime?: string | null;
  allDay: boolean;
  location?: string | null;
}

type Detail = { kind: 'task'; id: string } | { kind: 'event'; id: string } | null;

/**
 * A day, task or event the agent asked to show (`show_calendar`). Both
 * layouts bring it into view; `seq` grows with every request so a repeat
 * of the same target is applied again.
 */
export interface FocusRequest {
  seq: number;
  /** `YYYY-MM-DD`, or null to use the task's due day or the event's day. */
  day: string | null;
  taskId: string | null;
  eventId: string | null;
}

/** The day a focus request is about: its date, else the task's due day or the event's day. */
export function focusDayKey(request: FocusRequest, tasks: readonly TodoItem[], events: readonly CalendarEvent[]): string | null {
  if (request.day) return request.day;
  if (request.taskId) return storedDayKey(tasks.find(t => t.id === request.taskId)?.dueDate);
  if (request.eventId) return storedDayKey(events.find(e => e.id === request.eventId)?.startTime);
  return null;
}

interface TasksStoreValue {
  tasks: TodoItem[];
  events: CalendarEvent[];
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  createTask: (input: NewTaskInput) => Promise<TodoItem | null>;
  createEvent: (input: NewEventInput) => Promise<CalendarEvent | null>;
  /** Resolves true when saved (or nothing changed), false when rolled back. */
  updateTask: (id: string, patch: TaskPatch) => Promise<boolean>;
  /** Hides the task at once; it is deleted when the undo window ends. */
  deleteTask: (id: string) => void;
  addSubtask: (taskId: string, title: string) => Promise<boolean>;
  toggleSubtask: (taskId: string, subtaskId: string) => Promise<boolean>;
  renameSubtask: (taskId: string, subtaskId: string, title: string) => Promise<boolean>;
  deleteSubtask: (taskId: string, subtaskId: string) => Promise<boolean>;
  /** Ring the task's reminder again in `minutes`. */
  snoozeReminder: (taskId: string, minutes: number) => Promise<boolean>;
  updateEvent: (id: string, patch: EventPatch) => Promise<boolean>;
  deleteEvent: (id: string) => void;
  openTask: (id: string) => void;
  openEvent: (id: string) => void;
  /** The task or event whose detail sheet is open (rendered by `TasksView`). */
  detailTask: TodoItem | null;
  detailEvent: CalendarEvent | null;
  closeDetail: () => void;
  /** Latest agent request to show a day, task or event. */
  focusRequest: FocusRequest | null;
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
  const [focusRequest, setFocusRequest] = useState<FocusRequest | null>(null);
  const focusSeq = useRef(0);
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
    // Empty optional values clear the field (sent as `clear`); a title is required.
    if (requested.dueDate !== undefined && !requested.dueDate) requested.dueDate = null;
    if (requested.reminder !== undefined && !requested.reminder) requested.reminder = null;
    if (requested.project !== undefined) requested.project = requested.project?.trim() || null;
    if (requested.title !== undefined && !requested.title.trim()) delete requested.title;
    const changes = changedFields(current, requested as Partial<TodoItem>) as TaskPatch;
    if (changes.dueDate && sameMoment(changes.dueDate, current.dueDate)) delete changes.dueDate;
    if (changes.tags !== undefined && sameTags(changes.tags, current.tags)) delete changes.tags;
    if (Object.keys(changes).length === 0) return true;
    // A new or removed reminder starts over (as the backend does).
    const optimistic: Partial<TodoItem> = changes.reminder !== undefined
      ? { ...changes, reminderFiredAt: null, snoozedUntil: null }
      : changes;
    return runTaskOp(
      id,
      optimistic,
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

  const createEvent = useCallback(async (input: NewEventInput): Promise<CalendarEvent | null> => {
    const args: Record<string, unknown> = {
      title: input.title.trim(),
      startTime: input.startTime,
      allDay: input.allDay,
      source: 'user',
    };
    if (input.endTime) args.endTime = input.endTime;
    if (input.location?.trim()) args.location = input.location.trim();
    try {
      const event = await invoke<CalendarEvent>('create_event', args);
      dispatch({ type: 'insertEvent', event });
      return event;
    } catch (err) {
      notify.error('Could not add the event', { description: errorText(err) });
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

  const renameSubtask = useCallback(async (taskId: string, subtaskId: string, title: string): Promise<boolean> => {
    const current = latest.current.tasks.find(t => t.id === taskId);
    const text = title.trim();
    if (!current || !text || isTempSubtaskId(subtaskId)) return false;
    if (current.subtasks.find(s => s.id === subtaskId)?.title === text) return true;
    return runTaskOp(
      taskId,
      { subtasks: current.subtasks.map(s => (s.id === subtaskId ? { ...s, title: text } : s)) },
      () => invoke<TodoItem>('rename_subtask', { taskId, subtaskId, title: text }),
      'Could not rename the subtask',
    );
  }, [runTaskOp]);

  const snoozeReminder = useCallback(async (taskId: string, minutes: number): Promise<boolean> => {
    try {
      const task = await invoke<TodoItem>('snooze_reminder', { taskId, minutes });
      dispatch({ type: 'insertTask', task });
      return true;
    } catch (err) {
      notify.error('Could not snooze the reminder', { description: errorText(err) });
      return false;
    }
  }, []);

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
    // Empty optional values clear the field; start and title are required.
    if (requested.endTime !== undefined && !requested.endTime) requested.endTime = null;
    if (requested.location !== undefined) requested.location = requested.location?.trim() || null;
    if (requested.startTime !== undefined && !requested.startTime) delete requested.startTime;
    if (requested.title !== undefined && !requested.title.trim()) delete requested.title;
    const changes = changedFields(current, requested as Partial<CalendarEvent>) as EventPatch;
    if (changes.startTime !== undefined && sameMoment(changes.startTime, current.startTime)) delete changes.startTime;
    if (changes.endTime && sameMoment(changes.endTime, current.endTime)) delete changes.endTime;
    if (Object.keys(changes).length === 0) return true;
    const opId = nextOp();
    dispatch({ type: 'begin', op: { opId, kind: 'event', id, patch: changes as Partial<CalendarEvent> } });
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
    try {
      // Literal command names: the agent coverage gate checks every invoke.
      if (entry.kind === 'task') await invoke('delete_task', { id: entry.id });
      else await invoke('delete_event', { id: entry.id });
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

  // The agent pointed at a day, task or event. Reload first: the agent
  // usually just wrote the record, and a sheet opened for an id the list
  // does not have yet would be closed as deleted.
  useNavigationTarget('calendar', target => {
    const seq = ++focusSeq.current;
    void refresh().then(() => {
      if (seq !== focusSeq.current) return;
      setFocusRequest({ seq, day: target.date, taskId: target.taskId, eventId: target.eventId });
      if (target.taskId) setDetail({ kind: 'task', id: target.taskId });
      else if (target.eventId) setDetail({ kind: 'event', id: target.eventId });
    });
  });

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
    createEvent,
    updateTask,
    deleteTask,
    addSubtask,
    toggleSubtask,
    renameSubtask,
    deleteSubtask,
    snoozeReminder,
    updateEvent,
    deleteEvent,
    openTask,
    openEvent,
    detailTask,
    detailEvent,
    closeDetail,
    focusRequest,
  }), [tasks, events, loading, error, refresh, createTask, createEvent, updateTask, deleteTask, addSubtask, toggleSubtask, renameSubtask, deleteSubtask, snoozeReminder, updateEvent, deleteEvent, openTask, openEvent, detailTask, detailEvent, closeDetail, focusRequest]);

  return <TasksStoreContext.Provider value={value}>{children}</TasksStoreContext.Provider>;
}
