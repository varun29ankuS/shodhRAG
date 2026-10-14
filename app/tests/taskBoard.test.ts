/**
 * A workspace's task board and the workspace field of tasks. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/taskBoard.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { adjacentStatus, boardColumns, columnOf, workspaceChoices, workspaceName } from '../src/features/tasks/board.ts';
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
