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
  settingsSectionOf,
  attentionCount,
  attentionLabel,
  countLabel,
  panelState,
  tabItems,
  tabOf,
  AUTOMATION_YIELD_ID,
  EMPTY_COPY,
  TAB_LABEL,
  TAB_RULES,
  INSTALL_SCAN_READER,
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

test('automation items are parsed, not skipped as a newer build kind', () => {
  const items = parseItems([
    { id: 'repeat-hold:j1', kind: 'automation', status: 'needs_you', title: 'Submit timesheet stopped repeating' },
  ]);
  assert.deepEqual(items.map(i => i.kind), ['automation']);
});

test('a repeating task question offers its three answers and no dismiss; a hold offers Resume', () => {
  const question = { ...item('repeat:j1', 'reminder', 'needs_you'), data: { taskId: 't1', jobId: 'j1', choices: ['make', 'skip', 'mark_done'] } };
  assert.deepEqual(actionsFor(question), { primary: null, secondary: null, choices: ['make', 'skip', 'mark_done'] });
  const hold = { ...item('repeat-hold:j1', 'automation', 'needs_you'), data: { taskId: 't1', jobId: 'j1', fix: 'resume' } };
  assert.deepEqual(actionsFor(hold), { primary: null, secondary: null, choices: ['resume'] });
  // A plain reminder keeps Dismiss.
  assert.deepEqual(actionsFor(item('reminder:t1', 'reminder', 'needs_you')), { primary: null, secondary: 'dismiss' });
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

test('the scan reader offer installs while it waits; other installs and finished offers only dismiss', () => {
  const offer = { ...item('install:scan-reader', 'install', 'needs_you'), data: { action: INSTALL_SCAN_READER } };
  assert.deepEqual(actionsFor(offer), { primary: 'install', secondary: 'dismiss' });
  assert.deepEqual(actionsFor({ ...offer, status: 'working' }), { primary: null, secondary: null });
  assert.deepEqual(actionsFor({ ...offer, status: 'done' }), { primary: null, secondary: 'dismiss' });
  assert.deepEqual(actionsFor(item('install:mcp', 'install', 'needs_you')), { primary: null, secondary: 'dismiss' });
  assert.equal(attentionCount([offer]), 1);
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
  assert.equal(attentionCount([
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

test('Open on a memory suggestion lands on Settings → Memory, other links on their view', () => {
  const settings = { view: 'settings', target: null };
  assert.equal(settingsSectionOf({ kind: 'memory', link: settings }), 'memory');
  assert.equal(settingsSectionOf({ kind: 'memory', link: null }), null);
  assert.equal(settingsSectionOf({ kind: 'install', link: settings }), null);
  assert.equal(settingsSectionOf({ kind: 'model', link: settings }), 'models', 'a failing model opens Settings → Model');
  assert.equal(settingsSectionOf({ kind: 'model', link: null }), null);
  assert.equal(settingsSectionOf({ kind: 'reminder', link: { view: 'tasks', target: null } }), null);
});

test('the Inbox reloads when the calendar changes, so a done task stops counting its reminder', () => {
  const source = readFileSync(new URL('../src/features/inbox/InboxButton.tsx', import.meta.url), 'utf8');
  assert.match(source, /listen\('calendar-changed', \(\) => void load\(\)\)/);
});

// The Attention and Info tabs (spec §11, PR 2).

const withData = (i: InboxItem, data: Record<string, unknown>): InboxItem => ({ ...i, data });

test('every kind and status lands in its tab', () => {
  // [working, needs_you, done, failed], written out: a failure is Attention only
  // when it stops something the user owns.
  const expected: Record<InboxItem['kind'], [string, string, string, string]> = {
    approval: ['info', 'attention', 'info', 'info'],
    memory: ['info', 'attention', 'info', 'info'],
    reminder: ['info', 'attention', 'info', 'info'],
    indexing: ['info', 'attention', 'info', 'info'],
    tables: ['info', 'attention', 'info', 'info'],
    citation_graph: ['info', 'attention', 'info', 'info'],
    export: ['info', 'attention', 'info', 'info'],
    install: ['info', 'attention', 'info', 'info'],
    connector: ['info', 'attention', 'info', 'attention'],
    model: ['info', 'attention', 'info', 'attention'],
    automation: ['info', 'attention', 'info', 'info'],
  };
  const statuses: InboxItem['status'][] = ['working', 'needs_you', 'done', 'failed'];
  for (const [kind, tabs] of Object.entries(expected) as [InboxItem['kind'], string[]][]) {
    statuses.forEach((status, i) => {
      assert.equal(tabOf(item(`${kind}:${status}`, kind, status)), tabs[i], `${kind} ${status}`);
    });
  }
});

test('a folder that could not be synced or indexed is attention, and a partial sync is info', () => {
  const failed = item('indexing:s1', 'indexing', 'failed');
  assert.equal(tabOf(withData(failed, { failure: 'sync', folder: 'Papers' })), 'attention');
  assert.equal(tabOf(withData(failed, { failure: 'index' })), 'attention', 'a first index that failed outright');
  assert.equal(tabOf(withData(failed, { quiet: false, failure: null })), 'info', 'some files could not be read');
});

test('a one-off connector failure is info; one that stays stopped is attention', () => {
  assert.equal(tabOf(withData(item('slack:model:T1', 'connector', 'failed'), { connector: 'slack', oneOff: true })), 'info');
  assert.equal(tabOf(withData(item('github:rate-limit', 'connector', 'failed'), { connector: 'github', oneOff: true })), 'info');
  assert.equal(tabOf(withData(item('slack:connection:T1', 'connector', 'failed'), { connector: 'slack' })), 'attention');
});

test('a repeating task question and a hold land in attention', () => {
  const question = withData(item('repeat:j1', 'reminder', 'needs_you'), {
    taskId: 't1', jobId: 'j1', choices: ['make', 'skip', 'mark_done'], occurrence: '2026-11-24T17:00:00Z', missed: 0,
  });
  const hold = withData(item('repeat-hold:j1', 'automation', 'needs_you'), { taskId: 't1', jobId: 'j1', fix: 'resume' });
  assert.equal(tabOf(question), 'attention');
  assert.equal(tabOf(hold), 'attention');
});

test('an automation that yields to chat lands in info, also after a restart', () => {
  const paused = withData(item(AUTOMATION_YIELD_ID, 'automation', 'working'), { paused: 'chat', resumesAt: '2026-11-24T00:00:00Z' });
  assert.equal(AUTOMATION_YIELD_ID, 'automation:yield');
  assert.equal(tabOf(paused), 'info');
  assert.equal(tabOf({ ...paused, status: 'failed' }), 'info');
});

test('an item stopped at close never adds to attention', () => {
  const kinds: InboxItem['kind'][] = ['indexing', 'tables', 'citation_graph', 'install', 'export', 'automation'];
  const stopped = kinds.flatMap(kind => [
    item(`${kind}:a`, kind, 'failed'),
    withData(item(`${kind}:b`, kind, 'failed'), { quiet: true }),
  ]);
  assert.equal(attentionCount(stopped), 0);
});

test('the attention tab and the bell agree, and each tab keeps the given order', () => {
  const list = [
    item('a', 'approval', 'needs_you'),
    item('m', 'memory', 'needs_you'),
    item('x', 'model', 'failed'),
    item('e', 'export', 'done'),
    item('t', 'tables', 'failed'),
    item('i', 'indexing', 'working'),
  ];
  assert.equal(attentionCount(list), 3);
  assert.deepEqual(tabItems(list, 'attention').map(i => i.id), ['a', 'm', 'x']);
  assert.deepEqual(tabItems(list, 'info').map(i => i.id), ['e', 't', 'i']);
  assert.equal(attentionLabel(list, 99), '3');
  assert.equal(attentionLabel(list, 2), '2+');
});

test('count labels cap at their width', () => {
  assert.equal(countLabel(0, 99), '0');
  assert.equal(countLabel(99, 99), '99');
  assert.equal(countLabel(100, 99), '99+');
  assert.equal(countLabel(10, 9), '9+', 'the bell');
});

test('past the memory cap the attention count says so, never a smaller number', () => {
  const capped = Array.from({ length: 50 }, (_, n) =>
    withData(item(`memory:p${n}`, 'memory', 'needs_you'), { suggestionId: `p${n}`, capped: true }));
  assert.equal(attentionLabel(capped, 99), '50+');
  assert.equal(attentionLabel([...capped, item('a', 'approval', 'needs_you')], 99), '51+');
  assert.equal(attentionLabel(capped, 9), '9+', 'the bell');
  const uncapped = capped.map(i => withData(i, { suggestionId: i.data.suggestionId }));
  assert.equal(attentionLabel(uncapped, 99), '50');
  // A capped flag on anything but a memory suggestion means nothing.
  assert.equal(attentionLabel([withData(item('r', 'reminder', 'needs_you'), { capped: true })], 99), '1');
});

test('loading is never shown as empty', () => {
  assert.equal(panelState({ loaded: false, error: null, count: 0 }), 'loading');
  assert.equal(panelState({ loaded: true, error: null, count: 0 }), 'empty');
  assert.equal(panelState({ loaded: true, error: 'x', count: 0 }), 'error');
  assert.equal(panelState({ loaded: true, error: 'x', count: 2 }), 'list');
  assert.equal(panelState({ loaded: false, error: 'x', count: 0 }), 'error');
  assert.equal(panelState({ loaded: true, error: null, count: 1 }), 'list');
});

test('empty states use the words of the spec', () => {
  assert.equal(`${EMPTY_COPY.attention.title} ${EMPTY_COPY.attention.detail}`, 'Nothing needs your attention. Shodh puts approvals, reminders and suggestions here.');
  assert.equal(`${EMPTY_COPY.info.title} ${EMPTY_COPY.info.detail}`, 'Nothing new. Finished work and updates show here.');
});

test('no UI text says Signal or Noise', () => {
  assert.deepEqual(TAB_LABEL, { attention: 'Attention', info: 'Info' });
  const texts = [
    ...Object.values(TAB_LABEL),
    ...Object.values(TAB_RULES),
    ...Object.values(EMPTY_COPY).flatMap(c => [c.title, c.detail]),
  ];
  for (const text of texts) assert.doesNotMatch(text, /signal|noise/i);
});

const inboxButton = () => read('features/inbox/InboxButton.tsx');

test('the panel has two tabs and opens on attention; only Attention has a count', () => {
  const source = inboxButton();
  assert.match(source, /role="tablist"/);
  assert.match(source, /role="tab"/);
  assert.match(source, /role="tabpanel"/);
  assert.match(source, /setTab\('attention'\)/);
  assert.match(source, /attentionLabel\(visible, 99\)/);
  assert.match(source, />\s*Show Info\s*</, 'the empty Attention link names no number');
  assert.doesNotMatch(source, /Show Info \(/);
});

test('the badge counts attention', () => {
  const source = inboxButton();
  assert.match(source, /attentionLabel\(visible, 9\)/);
  assert.doesNotMatch(source, /waitingCount/);
  assert.match(source, /need your attention/);
});

test('the inbox action row never wraps', () => {
  const source = inboxButton();
  assert.doesNotMatch(source, /flex-wrap/);
  const action = /const ACTION =\s*'([^']*)'/.exec(source);
  assert.ok(action, 'the ACTION class constant');
  assert.match(action[1], /whitespace-nowrap/);
});

test('the why popover neither closes nor drives the inbox', () => {
  const source = inboxButton();
  assert.match(source, /label="Why is this here\?"/);
  assert.ok((source.match(/insideOtherDialog/g) ?? []).length >= 3, 'imported, and used by the mousedown and key handlers');
});

test('the empty state is never shown before the first load', () => {
  const source = inboxButton();
  assert.match(source, /panelState\(/);
  assert.match(source, /'loading'/);
  assert.doesNotMatch(source, /Nothing is waiting for you\./);
});

test('the inbox is the last control of the top bar', () => {
  const app = read('App-SplitView.tsx');
  const once = (needle: string) => {
    const at = app.indexOf(needle);
    assert.ok(at >= 0 && app.indexOf(needle, at + 1) < 0, `${needle} occurs once`);
    return at;
  };
  const menu = once('<ChatActionsMenu');
  const sources = once('librarySourcesChip(searched)');
  const inbox = once('<InboxButton');
  assert.ok(menu < sources && sources < inbox, 'Ask actions, then Sources, then the Inbox');
  // After the Inbox's element, the next tag closes the group: nothing sits to its right.
  const after = app.slice(app.indexOf('/>', inbox) + 2);
  assert.match(after, /^\s*<\/div>/);
});
