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
  focusZone,
  inboxCounts,
  keyAction,
  panelNav,
  panelState,
  pointerDownCloses,
  requestGate,
  retryButton,
  ATTENTION_MAX,
  LIST_SELECTOR,
  ROW_SELECTOR,
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

test('a whats_new item parses and lands in info', () => {
  const items = parseItems([
    { id: 'whats_new:files:s1:20261123', kind: 'whats_new', status: 'done', title: '3 new files in Research Papers', link: { view: 'library', target: { kind: 'library', section: 'whats_new' } } },
  ]);
  assert.deepEqual(items.map(i => i.kind), ['whats_new']);
  assert.equal(tabOf(items[0]), 'info');
  assert.equal(tabOf({ ...items[0], status: 'failed' }), 'info');
  assert.deepEqual(actionsFor(items[0]), { primary: null, secondary: 'dismiss' });
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
  assert.match(read('features/agent/CodeBranchChip.tsx'), /Confirm discard/);
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
    whats_new: ['info', 'attention', 'info', 'info'],
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
  assert.equal(attentionLabel(list), '3');
  const counts = inboxCounts({ loaded: true, error: null, items: list });
  assert.deepEqual(counts, { pill: '3', bell: '3', bellAria: 'Inbox, 3 need your attention' });
});

test('count labels cap at their width', () => {
  assert.equal(countLabel(0, 99), '0');
  assert.equal(countLabel(99, 99), '99');
  assert.equal(countLabel(100, 99), '99+');
  assert.equal(countLabel(10, 9), '9+');
  assert.equal(ATTENTION_MAX, 99);
});

test('the bell shows exactly what the Attention tab shows, past 9 and past 99', () => {
  for (const n of [1, 10, 99, 100, 250]) {
    const list = Array.from({ length: n }, (_, i) => item(`a${i}`, 'approval', 'needs_you'));
    const counts = inboxCounts({ loaded: true, error: null, items: list });
    assert.equal(counts.bell, counts.pill, `${n} items`);
    assert.equal(counts.pill, n > 99 ? '99+' : String(n));
  }
  assert.equal(inboxCounts({ loaded: true, error: null, items: [item('a', 'approval', 'needs_you')] }).bellAria, 'Inbox, 1 needs your attention');
});

test('past the memory cap the attention count says so, on the bell and the tab', () => {
  const capped = Array.from({ length: 50 }, (_, n) =>
    withData(item(`memory:p${n}`, 'memory', 'needs_you'), { suggestionId: `p${n}`, capped: true }));
  const counts = inboxCounts({ loaded: true, error: null, items: capped });
  assert.equal(counts.pill, '50+');
  assert.equal(counts.bell, '50+');
  assert.equal(counts.bellAria, 'Inbox, 50+ need your attention');
  assert.equal(attentionLabel([...capped, item('a', 'approval', 'needs_you')]), '51+');
  const uncapped = capped.map(i => withData(i, { suggestionId: i.data.suggestionId }));
  assert.equal(attentionLabel(uncapped), '50');
  // A capped flag on anything but a memory suggestion means nothing.
  assert.equal(attentionLabel([withData(item('r', 'reminder', 'needs_you'), { capped: true })]), '1');
});

test('no count before the first answer, or beside a failed load with nothing to count', () => {
  const one = [item('a', 'approval', 'needs_you')];
  assert.deepEqual(inboxCounts({ loaded: false, error: null, items: [] }), { pill: null, bell: null, bellAria: 'Inbox' });
  assert.deepEqual(inboxCounts({ loaded: true, error: 'x', items: [] }), { pill: null, bell: null, bellAria: 'Inbox' });
  // A refresh that failed keeps the last list, and its count.
  assert.equal(inboxCounts({ loaded: true, error: 'x', items: one }).pill, '1');
  // Loaded and nothing waiting: the tab says 0, the bell shows nothing.
  assert.deepEqual(inboxCounts({ loaded: true, error: null, items: [] }), { pill: '0', bell: null, bellAria: 'Inbox' });
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

// The panel's behaviour as pure functions (this repo has no DOM test harness: its
// tests run under Node's strip-types runner, which cannot load .tsx).

test('every open starts on Attention, from the bell or the palette, before the first frame', () => {
  const onInfo = { open: false, tab: 'info' as const };
  assert.deepEqual(panelNav(onInfo, { type: 'toggle' }), { open: true, tab: 'attention' });
  assert.deepEqual(panelNav(onInfo, { type: 'open' }), { open: true, tab: 'attention' });
  const shown = panelNav({ open: true, tab: 'attention' }, { type: 'show', tab: 'info' });
  assert.deepEqual(shown, { open: true, tab: 'info' });
  assert.deepEqual(panelNav(shown, { type: 'toggle' }), { open: false, tab: 'info' }, 'closing keeps the tab until the next open');
  assert.deepEqual(panelNav(panelNav(shown, { type: 'close' }), { type: 'toggle' }), { open: true, tab: 'attention' });
  const closed = { open: false, tab: 'attention' as const };
  assert.equal(panelNav(closed, { type: 'close' }), closed, 'closing a closed panel changes nothing');
});

// A DOM stand-in: an element with the selectors it matches, inside or outside the panel.
interface Fake {
  name: string;
  matches: string[];
  closest(selector: string): unknown;
}
const el = (name: string, ...matches: string[]): Fake => ({
  name,
  matches,
  closest: (selector: string) => (selector.split(',').some(s => matches.includes(s.trim())) ? {} : null),
});
const box = (...inside: Fake[]) => ({ contains: (n: Fake | null) => n !== null && inside.includes(n) });

const row = el('row', ROW_SELECTOR, LIST_SELECTOR);
const rowButton = el('Approve', LIST_SELECTOR);
const tabButton = el('Info tab', '[role="tab"]');
const why = el('Why is this here?');
const showInfo = el('Show Info');
const tryAgain = el('Try again');
const popover = el('pop-up', '[role="dialog"]');
const panel = box(row, rowButton, tabButton, why, showInfo, tryAgain);

test('keys act on an item only from the item list', () => {
  assert.equal(focusZone(row, panel), 'row');
  assert.equal(focusZone(rowButton, panel), 'item');
  for (const other of [tabButton, why, showInfo, tryAgain]) assert.equal(focusZone(other, panel), 'panel', other.name);
  assert.equal(focusZone(popover, panel), 'outside', 'the pop-up is a portal outside the panel');
  assert.equal(focusZone(null, panel), 'outside');

  for (const command of ['primary', 'secondary', 'undo'] as const) {
    assert.equal(keyAction(command, 'row'), command);
    assert.equal(keyAction(command, 'item'), command);
    assert.equal(keyAction(command, 'panel'), null, `${command} on a tab, Why, Show Info or Try again`);
    assert.equal(keyAction(command, 'outside'), null, `${command} in the pop-up`);
  }
  assert.equal(keyAction('open', 'row'), 'open');
  assert.equal(keyAction('open', 'item'), null, 'Enter on a button presses that button');
  assert.equal(keyAction('open', 'panel'), null);
  for (const command of ['next', 'previous'] as const) {
    assert.equal(keyAction(command, 'panel'), command, 'J/K reach the list from a tab');
    assert.equal(keyAction(command, 'outside'), null);
  }
});

test('A on the Info tab never approves an approval in Attention', () => {
  const command = inboxCommand({ key: 'a' });
  assert.equal(command, 'primary');
  assert.equal(keyAction(command!, focusZone(tabButton, panel)), null);
});

test('a press in the Why pop-up keeps the Inbox open; a press elsewhere outside closes it', () => {
  const bell = el('bell');
  const root = box(bell, row, rowButton, tabButton, why);
  const page = el('page');
  assert.equal(pointerDownCloses(popover, root), false);
  assert.equal(pointerDownCloses(row, root), false);
  assert.equal(pointerDownCloses(bell, root), false, 'the bell toggles it itself');
  assert.equal(pointerDownCloses(page, root), true);
  assert.equal(pointerDownCloses(null, root), true, 'a press on the document itself');
  assert.equal(pointerDownCloses(page, null), false, 'no panel, nothing to close');
});

test('an older Inbox read never overwrites a newer one', () => {
  const gate = requestGate();
  const first = gate.begin();
  const second = gate.begin();
  assert.equal(gate.isLatest(first), false, 'the first answer arrives late and is dropped');
  assert.equal(gate.isLatest(second), true);
  const third = gate.begin();
  assert.equal(gate.isLatest(second), false);
  assert.equal(gate.isLatest(third), true);
});

test('Try again shows that it is trying, and cannot be pressed twice', () => {
  assert.deepEqual(retryButton(false), { label: 'Try again', disabled: false });
  assert.deepEqual(retryButton(true), { label: 'Trying again…', disabled: true });
});

test('the inbox action row never wraps', () => {
  // Wrapping is layout, which Node cannot measure; this pins the classes that forbid it
  // (the coordinator checks the rendered row over CDP).
  const source = read('features/inbox/InboxButton.tsx');
  assert.doesNotMatch(source, /flex-wrap/);
  const action = /const ACTION =\s*'([^']*)'/.exec(source);
  assert.ok(action, 'the ACTION class constant');
  assert.match(action[1], /whitespace-nowrap/);
});

test('work stopped at close, as Rust stores it, lands in Info', () => {
  // Written by the Rust test `work_stopped_at_close_matches_the_fixture_the_inbox_tabs_read`
  // (app/src-tauri/src/inbox_commands.rs), which asserts recover() makes exactly this.
  const fixture = JSON.parse(readFileSync(new URL('./fixtures/inbox/stopped-at-close.json', import.meta.url), 'utf8'));
  const items = parseItems(fixture);
  assert.equal(items.length, 2, 'both rows parse');
  assert.deepEqual(items.map(i => [i.kind, i.status]), [['citation_graph', 'failed'], ['indexing', 'failed']]);
  for (const i of items) assert.equal(tabOf(i), 'info', i.id);
  assert.equal(attentionCount(items), 0);
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
