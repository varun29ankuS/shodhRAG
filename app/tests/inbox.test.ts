/**
 * The Inbox's items, actions and keys, and which actions still ask first. Run
 * with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/inbox.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  STATUS_TONE,
  actionsFor,
  inboxCommand,
  isUndoable,
  keepSelection,
  moveSelection,
  parseItems,
  waitingCount,
} from '../src/features/inbox/model.ts';
import type { InboxItem } from '../src/features/inbox/model.ts';

function item(id: string, kind: InboxItem['kind'], status: InboxItem['status']): InboxItem {
  return { id, kind, status, title: id, detail: null, link: null, data: {}, createdAt: '', updatedAt: '' };
}

test('items from the backend are parsed; unknown kinds and malformed rows are skipped', () => {
  const items = parseItems([
    {
      id: 'approval:s1:a',
      kind: 'approval',
      status: 'needs_you',
      title: 'Write notes.md',
      detail: 'The assistant waits for your approval (write_file).',
      link: { view: 'ask', target: { kind: 'conversation', conversationId: 'c1' } },
      data: { sessionId: 's1', stepId: 'a' },
      createdAt: '2026-10-07T09:00:00.000Z',
      updatedAt: '2026-10-07T09:00:00.000Z',
    },
    { id: 'x', kind: 'teleport', status: 'done', title: 'From a newer build' },
    { id: 'y', kind: 'export', status: 'maybe', title: 'Bad status' },
    { kind: 'export', status: 'done', title: 'No id' },
    { id: 'export:1', kind: 'export', status: 'done', title: 'Export ready', link: { view: 'library' } },
  ]);
  assert.deepEqual(items.map(i => i.id), ['approval:s1:a', 'export:1']);
  assert.deepEqual(items[0].data, { sessionId: 's1', stepId: 'a' });
  assert.deepEqual(items[1].link, { view: 'library', target: null });
  assert.equal(items[1].detail, null);
  assert.deepEqual(parseItems(null), []);
});

test('each kind offers its actions; running work offers none', () => {
  assert.deepEqual(actionsFor(item('a', 'approval', 'needs_you')), { primary: 'approve', secondary: 'deny' });
  assert.deepEqual(actionsFor(item('m', 'memory', 'needs_you')), { primary: 'accept', secondary: 'dismiss' });
  assert.deepEqual(actionsFor(item('r', 'reminder', 'needs_you')), { primary: null, secondary: 'dismiss' });
  assert.deepEqual(actionsFor(item('i', 'indexing', 'working')), { primary: null, secondary: null });
  assert.deepEqual(actionsFor(item('i', 'indexing', 'failed')), { primary: null, secondary: 'dismiss' });
  assert.deepEqual(actionsFor(item('e', 'export', 'done')), { primary: null, secondary: 'dismiss' });
  assert.equal(isUndoable('dismiss'), true);
  assert.equal(isUndoable('deny'), false, 'the assistant moves on after a denial');
});

test('status colours use the tokens: blue working, amber needs you, green done, red failed only', () => {
  assert.equal(STATUS_TONE.working.dot, 'bg-shodh-info');
  assert.equal(STATUS_TONE.needs_you.dot, 'bg-shodh-warning');
  assert.equal(STATUS_TONE.done.dot, 'bg-shodh-success');
  assert.equal(STATUS_TONE.failed.dot, 'bg-shodh-error');
  const reds = Object.entries(STATUS_TONE).filter(([, tone]) => tone.dot.includes('error') || tone.text.includes('error'));
  assert.deepEqual(reds.map(([status]) => status), ['failed']);
});

test('the bell counts only what waits on the user', () => {
  assert.equal(waitingCount([
    item('a', 'approval', 'needs_you'),
    item('m', 'memory', 'needs_you'),
    item('i', 'indexing', 'working'),
    item('e', 'export', 'done'),
    item('f', 'tables', 'failed'),
  ]), 2);
});

test('keys: J/K and arrows move, Enter opens, A, D and U act', () => {
  const key = (k: string, mods: Partial<Record<'ctrlKey' | 'altKey' | 'metaKey', boolean>> = {}) => inboxCommand({ key: k, ...mods });
  assert.equal(key('j'), 'next');
  assert.equal(key('J'), 'next');
  assert.equal(key('ArrowDown'), 'next');
  assert.equal(key('k'), 'previous');
  assert.equal(key('ArrowUp'), 'previous');
  assert.equal(key('Enter'), 'open');
  assert.equal(key('a'), 'primary');
  assert.equal(key('d'), 'secondary');
  assert.equal(key('u'), 'undo');
  assert.equal(key('x'), null);
  assert.equal(key('a', { ctrlKey: true }), null, 'Ctrl+A is not Approve');
  assert.equal(key('d', { metaKey: true }), null);
  assert.equal(key('k', { altKey: true }), null);
  assert.equal(inboxCommand({ key: 'a' }, true), null, 'typing in a field is not a command');
});

test('the selection moves within the list and follows its item when the list changes', () => {
  assert.equal(moveSelection(0, 3, -1), 0);
  assert.equal(moveSelection(0, 3, 1), 1);
  assert.equal(moveSelection(2, 3, 1), 2);
  assert.equal(moveSelection(0, 0, 1), 0);
  const list = [item('a', 'export', 'done'), item('b', 'export', 'done'), item('c', 'export', 'done')];
  assert.equal(keepSelection('c', 2, [item('new', 'approval', 'needs_you'), ...list]), 3);
  // The selected item was dismissed: the selection stays at its place.
  assert.equal(keepSelection('b', 1, [list[0], list[2]]), 1);
  assert.equal(keepSelection('c', 2, [list[0]]), 0);
  assert.equal(keepSelection(null, 0, []), 0);
});

const read = (path: string) => readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8');

test('reversible removals no longer ask first; they offer Undo', () => {
  const converted: Record<string, RegExp> = {
    'hooks/useConversations.ts': /removeWithUndo\(/,
    'components/CalendarTodoPanel.tsx': /onRequestDelete=\{\(\) => doDelete\(task\)\}/,
    'features/workspaces/SourcesTab.tsx': /removeWithUndo\(/,
    'components/SuggestedMemories.tsx': /removeWithUndo\(/,
    'features/research/SnippetCard.tsx': /removeWithUndo\(/,
    'features/research/SnippetDetail.tsx': /removeWithUndo\(/,
    'features/visuals/actions.ts': /visualsApi\.restore\(root\)/,
    'components/ToolsSettings.tsx': /removeWithUndo\(/,
  };
  for (const [file, pattern] of Object.entries(converted)) {
    const source = read(file);
    assert.match(source, pattern, file);
    assert.doesNotMatch(source, /ConfirmDialog|Confirm remove|Delete permanently|setConfirm(ing|Delete|Remove)?\(/, file);
  }
  assert.doesNotMatch(read('features/visuals/actions.ts'), /\bask\(/);
});

test('irreversible actions still ask first', () => {
  // Clearing the index or the whole database.
  assert.match(read('components/DataManagement.tsx'), /setConfirmAction\('clear_docs'\)/);
  assert.match(read('components/DataManagement.tsx'), /setConfirmAction\('reset'\)/);
  // Discarding Code changes (a second press within the window).
  assert.match(read('features/agent/CodeModeSwitch.tsx'), /Confirm discard/);
  // Forgetting a memory with every earlier version, and stopping learning.
  assert.match(read('components/MemorySettings.tsx'), /title: 'Forget memory'/);
  assert.match(read('components/SuggestedMemories.tsx'), /title: 'Stop learning'/);
  // Deleting a workspace (its instruction history goes with it).
  assert.match(read('features/workspaces/WorkspacePage.tsx'), /title: 'Delete workspace'/);
  // Shortening audit retention deletes events for good.
  assert.match(read('features/audit/AuditView.tsx'), /title: 'Shorten audit retention\?'/);
});
