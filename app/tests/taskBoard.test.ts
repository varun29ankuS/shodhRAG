/**
 * A workspace's task board and the workspace field of tasks. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/taskBoard.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  adjacentStatus,
  ALL_WORKSPACES,
  boardColumns,
  columnOf,
  effectiveWorkspaceFilter,
  matchesWorkspaceFilter,
  parseWorkspaceFilter,
  sameWorkspaceFilter,
  serializeWorkspaceFilter,
  workspaceChoices,
  workspaceFilterChoices,
  workspaceName,
} from '../src/features/tasks/board.ts';
import type { WorkspaceFilter } from '../src/features/tasks/board.ts';
import { taskArgs } from '../src/features/tasks/taskStore.ts';
import type { TodoItem } from '../src/features/tasks/types.ts';

function task(id: string, extra: Partial<TodoItem> = {}): TodoItem {
  return {
    id,
    title: id,
    description: '',
    priority: 'medium',
    status: 'pending',
    tags: [],
    subtasks: [],
    source: 'user',
    createdAt: '2026-10-01T00:00:00Z',
    updatedAt: '2026-10-01T00:00:00Z',
    workspaceId: 'ws-1',
    ...extra,
  };
}

test('the board shows only its workspace, one column per backend status', () => {
  const columns = boardColumns(
    [
      task('a'),
      task('b', { status: 'in_progress' }),
      task('c', { status: 'completed' }),
      task('other', { workspaceId: 'ws-2' }),
      task('loose', { workspaceId: null }),
      task('unset', { workspaceId: undefined }),
    ],
    'ws-1',
  );
  assert.deepEqual(columns.map(c => c.status), ['pending', 'in_progress', 'completed']);
  assert.deepEqual(columns.map(c => c.label), ['To do', 'In progress', 'Done']);
  assert.deepEqual(columns.map(c => c.tasks.map(t => t.id)), [['a'], ['b'], ['c']]);
});

test('a status the board does not know shows under To do, never hidden', () => {
  assert.equal(columnOf({ status: 'waiting' }), 'pending');
  assert.equal(columnOf({ status: 'in_progress' }), 'in_progress');
  const [todo] = boardColumns([task('x', { status: 'waiting' })], 'ws-1');
  assert.deepEqual(todo.tasks.map(t => t.id), ['x']);
});

test('cards are ordered by due date, then priority, then newest', () => {
  const [todo] = boardColumns(
    [
      task('later', { dueDate: '2026-10-20' }),
      task('soon', { dueDate: '2026-10-05' }),
      task('low', { priority: 'low' }),
      task('high-old', { priority: 'high', createdAt: '2026-09-01T00:00:00Z' }),
      task('high-new', { priority: 'high', createdAt: '2026-09-02T00:00:00Z' }),
    ],
    'ws-1',
  );
  assert.deepEqual(todo.tasks.map(t => t.id), ['soon', 'later', 'high-new', 'high-old', 'low']);
});

test('the keyboard moves a card one column at a time and stops at the edges', () => {
  assert.equal(adjacentStatus('pending', 1), 'in_progress');
  assert.equal(adjacentStatus('in_progress', 1), 'completed');
  assert.equal(adjacentStatus('completed', 1), null);
  assert.equal(adjacentStatus('pending', -1), null);
  assert.equal(adjacentStatus('in_progress', -1), 'pending');
  assert.equal(adjacentStatus('waiting', 1), null);
});

test('a workspace reads as its name; a missing one reads as none', () => {
  const list = [{ id: 'ws-1', name: 'Thesis' }];
  assert.equal(workspaceName('ws-1', list), 'Thesis');
  assert.equal(workspaceName('ws-gone', list), null);
  assert.equal(workspaceName(null, list), null);
  assert.equal(workspaceName(undefined, list), null);
});

test('setting and clearing the workspace of a task', () => {
  assert.deepEqual(taskArgs('t1', { workspaceId: 'ws-1' }), { id: 't1', workspaceId: 'ws-1' });
  assert.deepEqual(taskArgs('t1', { workspaceId: null }), { id: 't1', clear: ['workspace_id'] });
  assert.deepEqual(taskArgs('t1', { workspaceId: '' }), { id: 't1', clear: ['workspace_id'] });
});

test('archived workspaces are offered only to the task already in one', () => {
  const list = [
    { id: 'ws-1', name: 'Thesis', archived: false },
    { id: 'ws-2', name: 'Old', archived: true },
  ];
  assert.deepEqual(workspaceChoices(list, null).map(w => w.id), ['ws-1']);
  assert.deepEqual(workspaceChoices(list, 'ws-2').map(w => w.id), ['ws-1', 'ws-2']);
});

test('the Tasks list workspace filter survives storage, and an id never reads as all or none', () => {
  const filters: WorkspaceFilter[] = [
    ALL_WORKSPACES,
    { kind: 'none' },
    { kind: 'workspace', id: 'ws-1' },
    { kind: 'workspace', id: 'all' },
    { kind: 'workspace', id: 'none' },
  ];
  for (const filter of filters) {
    assert.deepEqual(parseWorkspaceFilter(serializeWorkspaceFilter(filter)), filter);
  }
  for (const raw of [null, undefined, '', 'garbage', 'workspace:', 'all']) {
    assert.deepEqual(parseWorkspaceFilter(raw), ALL_WORKSPACES, String(raw));
  }
  assert.ok(sameWorkspaceFilter({ kind: 'workspace', id: 'a' }, { kind: 'workspace', id: 'a' }));
  assert.ok(!sameWorkspaceFilter({ kind: 'workspace', id: 'a' }, { kind: 'workspace', id: 'b' }));
  assert.ok(!sameWorkspaceFilter({ kind: 'none' }, ALL_WORKSPACES));
});

test("the workspace filter shows all, unfiled, or one workspace's tasks", () => {
  const tasks = [task('a', { workspaceId: 'ws-1' }), task('b', { workspaceId: null }), task('c', { workspaceId: undefined }), task('d', { workspaceId: 'ws-2' })];
  const shown = (filter: WorkspaceFilter) => tasks.filter(t => matchesWorkspaceFilter(t, filter)).map(t => t.id);
  assert.deepEqual(shown(ALL_WORKSPACES), ['a', 'b', 'c', 'd']);
  assert.deepEqual(shown({ kind: 'none' }), ['b', 'c']);
  assert.deepEqual(shown({ kind: 'workspace', id: 'ws-2' }), ['d']);
});

test('the filter offers live workspaces, archived ones holding tasks, and the chosen one', () => {
  const workspaces = [
    { id: 'live', archived: false },
    { id: 'old-with-tasks', archived: true },
    { id: 'old-empty', archived: true },
    { id: 'old-chosen', archived: true },
  ];
  const tasks = [task('a', { workspaceId: 'old-with-tasks' })];
  assert.deepEqual(workspaceFilterChoices(workspaces, tasks, ALL_WORKSPACES).map(w => w.id), ['live', 'old-with-tasks']);
  assert.deepEqual(
    workspaceFilterChoices(workspaces, tasks, { kind: 'workspace', id: 'old-chosen' }).map(w => w.id),
    ['live', 'old-with-tasks', 'old-chosen'],
  );
});

test('a remembered workspace that was deleted falls back to all, but only once the list has loaded', () => {
  const gone: WorkspaceFilter = { kind: 'workspace', id: 'deleted' };
  const listed = [{ id: 'ws-1' }];
  assert.deepEqual(effectiveWorkspaceFilter(gone, [], false), gone, 'still loading: kept');
  assert.deepEqual(effectiveWorkspaceFilter(gone, listed, true), ALL_WORKSPACES);
  const kept: WorkspaceFilter = { kind: 'workspace', id: 'ws-1' };
  assert.deepEqual(effectiveWorkspaceFilter(kept, listed, true), kept);
  assert.deepEqual(effectiveWorkspaceFilter({ kind: 'none' }, [], true), { kind: 'none' });
});
