/**
 * Workspace place tests: the open workspace, tab and table survive the Workspaces view
 * unmounting (a trip to Settings and back). Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/workspacePlace.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  forgetPlace,
  lastWorkspacePlace,
  openTableOf,
  rememberOpenTable,
  rememberWorkspacePlace,
} from '../src/features/workspaces/place.ts';

test('the workspace, its tab and its open table are found again after leaving', () => {
  assert.deepEqual(lastWorkspacePlace(), { workspaceId: null, tab: 'overview' });
  rememberWorkspacePlace({ workspaceId: 'ws-1', tab: 'tables' });
  rememberOpenTable('ws-1', 'table-a');
  rememberOpenTable('ws-2', 'table-b');
  // The view unmounts (Settings is shown) and mounts again.
  assert.deepEqual(lastWorkspacePlace(), { workspaceId: 'ws-1', tab: 'tables' });
  assert.equal(openTableOf('ws-1'), 'table-a');
  assert.equal(openTableOf('ws-2'), 'table-b');
  assert.equal(openTableOf('ws-3'), null);
});

test('back to the list or the table list is remembered too', () => {
  rememberOpenTable('ws-1', null);
  assert.equal(openTableOf('ws-1'), null);
  rememberWorkspacePlace({ workspaceId: null, tab: 'tables' });
  assert.deepEqual(lastWorkspacePlace(), { workspaceId: null, tab: 'overview' });
});

test('a deleted workspace or table is forgotten', () => {
  rememberWorkspacePlace({ workspaceId: 'ws-2', tab: 'tables' });
  rememberOpenTable('ws-2', 'table-b');
  forgetPlace({ tableId: 'table-b' });
  assert.equal(openTableOf('ws-2'), null);
  rememberOpenTable('ws-2', 'table-c');
  forgetPlace({ workspaceId: 'ws-2' });
  assert.equal(openTableOf('ws-2'), null);
  assert.deepEqual(lastWorkspacePlace(), { workspaceId: null, tab: 'overview' });
});
