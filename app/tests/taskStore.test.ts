/**
 * Optimistic task/event store. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/taskStore.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  changedFields,
  initialStore,
  isTempSubtaskId,
  pendingTaskRemovals,
  storeReducer,
  TEMP_SUBTASK_PREFIX,
  visibleEvents,
  visibleTasks,
} from '../src/features/tasks/taskStore.ts';
import type { StoreAction, StoreState } from '../src/features/tasks/taskStore.ts';
import type { CalendarEvent, TodoItem } from '../src/features/tasks/types.ts';

function task(id: string, extra: Partial<TodoItem> = {}): TodoItem {
  return {
    id,
    title: `Task ${id}`,
    description: '',
    dueDate: null,
    priority: 'medium',
    status: 'pending',
    tags: [],
    subtasks: [],
    project: null,
    source: 'user',
    sourceRef: null,
    createdAt: '2026-10-01T00:00:00Z',
    updatedAt: '2026-10-01T00:00:00Z',
    ...extra,
  };
}

function event(id: string, extra: Partial<CalendarEvent> = {}): CalendarEvent {
  return { id, title: `Event ${id}`, description: '', startTime: '2026-10-03T09:00', allDay: false, source: 'user', createdAt: '', ...extra };
}

const run = (actions: StoreAction[], from: StoreState = initialStore) => actions.reduce(storeReducer, from);
const loaded = run([{ type: 'snapshot', tasks: [task('a'), task('b')], events: [event('e')] }]);

test('an optimistic edit shows immediately and commits to the server record', () => {
  let s = run([{ type: 'begin', op: { opId: 1, kind: 'task', id: 'a', patch: { title: 'New' } } }], loaded);
  assert.equal(visibleTasks(s)[0].title, 'New');
  assert.equal(s.tasks[0].title, 'Task a', 'server state is untouched until commit');
  s = storeReducer(s, { type: 'commit', opId: 1, task: task('a', { title: 'New', updatedAt: 'later' }) });
  assert.equal(s.pending.length, 0);
  assert.equal(visibleTasks(s)[0].updatedAt, 'later');
});

test('a failed edit rolls back to the last server state', () => {
  const s = run([
    { type: 'begin', op: { opId: 1, kind: 'task', id: 'a', patch: { priority: 'high' } } },
    { type: 'rollback', opId: 1 },
  ], loaded);
  assert.equal(visibleTasks(s)[0].priority, 'medium');
  assert.equal(s.pending.length, 0);
});

test('a refetch during an edit does not undo it on screen', () => {
  const s = run([
    { type: 'begin', op: { opId: 1, kind: 'task', id: 'a', patch: { title: 'Mine' } } },
    // The calendar-changed refetch from another write lands before op 1 returns.
    { type: 'snapshot', tasks: [task('a', { status: 'completed' }), task('b')], events: [] },
  ], loaded);
  const a = visibleTasks(s).find(t => t.id === 'a')!;
  assert.equal(a.title, 'Mine');
  assert.equal(a.status, 'completed', 'fields not being edited follow the snapshot');
});

test('rolling back one of two edits keeps the other', () => {
  const s = run([
    { type: 'begin', op: { opId: 1, kind: 'task', id: 'a', patch: { title: 'T' } } },
    { type: 'begin', op: { opId: 2, kind: 'task', id: 'a', patch: { priority: 'low' } } },
    { type: 'rollback', opId: 1 },
  ], loaded);
  const a = visibleTasks(s)[0];
  assert.equal(a.title, 'Task a');
  assert.equal(a.priority, 'low');
});

test('later edits to the same field win until they resolve', () => {
  const s = run([
    { type: 'begin', op: { opId: 1, kind: 'task', id: 'a', patch: { title: 'one' } } },
    { type: 'begin', op: { opId: 2, kind: 'task', id: 'a', patch: { title: 'two' } } },
    { type: 'commit', opId: 1, task: task('a', { title: 'one' }) },
  ], loaded);
  assert.equal(visibleTasks(s)[0].title, 'two');
});

test('a pending delete hides the task without touching it; undo restores it', () => {
  let s = run([{ type: 'begin', op: { opId: 1, kind: 'task', id: 'a', remove: true } }], loaded);
  assert.deepEqual(visibleTasks(s).map(t => t.id), ['b']);
  assert.deepEqual([...pendingTaskRemovals(s)], ['a']);
  assert.equal(s.tasks.length, 2);
  // A refetch while waiting still contains the task; it stays hidden.
  s = storeReducer(s, { type: 'snapshot', tasks: [task('a'), task('b')], events: [] });
  assert.deepEqual(visibleTasks(s).map(t => t.id), ['b']);
  const undone = storeReducer(s, { type: 'rollback', opId: 1 });
  assert.deepEqual(visibleTasks(undone).map(t => t.id), ['a', 'b']);
  const committed = storeReducer(s, { type: 'commit', opId: 1, removed: true });
  assert.deepEqual(committed.tasks.map(t => t.id), ['b']);
});

test('event edits and deletes use the same layering', () => {
  let s = run([{ type: 'begin', op: { opId: 1, kind: 'event', id: 'e', patch: { title: 'Moved', startTime: '2026-10-04T09:00' } } }], loaded);
  assert.equal(visibleEvents(s)[0].startTime, '2026-10-04T09:00');
  s = storeReducer(s, { type: 'rollback', opId: 1 });
  assert.equal(visibleEvents(s)[0].startTime, '2026-10-03T09:00');
  s = run([
    { type: 'begin', op: { opId: 2, kind: 'event', id: 'e', remove: true } },
    { type: 'commit', opId: 2, removed: true },
  ], s);
  assert.equal(visibleEvents(s).length, 0);
});

test('created records are inserted once, newest task first', () => {
  const s = run([
    { type: 'insertTask', task: task('c') },
    { type: 'insertTask', task: task('c', { title: 'dupe' }) },
  ], loaded);
  assert.deepEqual(s.tasks.map(t => t.id), ['c', 'a', 'b']);
  assert.equal(s.tasks[0].title, 'dupe');
});

test('patches for tasks that vanished from the snapshot are ignored', () => {
  const s = run([
    { type: 'begin', op: { opId: 1, kind: 'task', id: 'a', patch: { title: 'x' } } },
    { type: 'snapshot', tasks: [task('b')], events: [] },
  ], loaded);
  assert.deepEqual(visibleTasks(s).map(t => t.id), ['b']);
});

test('changedFields keeps only real changes', () => {
  const current = task('a', { title: 'T', tags: ['x'], project: null });
  assert.deepEqual(changedFields(current, { title: 'T', tags: ['x'], project: undefined }), {});
  assert.deepEqual(changedFields(current, { title: 'U', tags: ['x', 'y'] }), { title: 'U', tags: ['x', 'y'] });
});

test('temporary subtask ids are recognisable', () => {
  assert.equal(isTempSubtaskId(`${TEMP_SUBTASK_PREFIX}1`), true);
  assert.equal(isTempSubtaskId('3f1c'), false);
});
