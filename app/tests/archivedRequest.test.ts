/**
 * The sidebar's "every chat is in an archived workspace" state opens the archived
 * list, whether Workspaces is open already or mounts after the navigation, and the
 * chat list only says "Loading" until the conversations were read. Run with Node
 * 22.6+:
 *   node --experimental-strip-types --test app/tests/archivedRequest.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { SHOW_ARCHIVED_WORKSPACES, requestArchivedWorkspaces, takeArchivedRequest } from '../src/features/workspaces/archivedRequest.ts';

test('a request is announced and taken once', () => {
  const seen: string[] = [];
  const target = { dispatchEvent: (e: Event) => { seen.push(e.type); return true; } };
  assert.equal(takeArchivedRequest(), false);
  requestArchivedWorkspaces(target);
  assert.deepEqual(seen, [SHOW_ARCHIVED_WORKSPACES]);
  assert.equal(takeArchivedRequest(), true);
  assert.equal(takeArchivedRequest(), false);
});

test('the chat list shows loading from the loaded flag, and an empty list links to the archived workspaces', () => {
  const history = readFileSync(new URL('../src/components/shell/ConversationHistory.tsx', import.meta.url), 'utf8');
  assert.match(history, /\{!loaded \? \(/);
  assert.match(history, /No chats outside archived workspaces\./);
  assert.match(history, /onClick=\{onShowArchived\}/);
  const conversations = readFileSync(new URL('../src/hooks/useConversations.ts', import.meta.url), 'utf8');
  assert.equal(conversations.match(/setLoaded\(true\)/g)?.length, 2);
});
