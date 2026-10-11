/**
 * Workspace logic tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/workspaces.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  answerScope,
  groupChatsByWorkspace,
  lineDiff,
  orderTemplates,
  parseDiff,
  pathWithin,
  scopeChip,
  sourceSummary,
  sortWorkspaces,
  workspaceForNewChat,
} from '../src/features/workspaces/model.ts';
import type { Workspace, WorkspaceTemplate } from '../src/features/workspaces/types.ts';

const counts = (folders = 0, files = 0, snippets = 0, papers = 0) => ({ folders, files, snippets, papers });

function workspace(id: string, extra: Partial<Workspace> = {}): Workspace {
  return {
    id,
    name: id.toUpperCase(),
    description: '',
    icon: 'folder',
    color: 'neutral',
    template: 'blank',
    pinned: false,
    archived: false,
    createdAt: '2026-10-01T00:00:00Z',
    updatedAt: '2026-10-01T00:00:00Z',
    lastActiveAt: null,
    instructionsVersion: 0,
    sourceCounts: counts(),
    ...extra,
  };
}

const chat = (id: string, updatedAt: string, workspaceId?: string, pinned = false) => ({ id, updatedAt, pinned, workspaceId });

test('chats group by workspace: pinned and recent workspaces first, No workspace last', () => {
  const workspaces = [
    workspace('thesis'),
    workspace('grant', { pinned: true }),
    workspace('old', { archived: true }),
    workspace('empty'),
  ];
  const groups = groupChatsByWorkspace(
    [
      chat('a', '2026-10-05T10:00:00Z', 'thesis'),
      chat('b', '2026-10-06T10:00:00Z', 'thesis'),
      chat('c', '2026-10-01T10:00:00Z', 'grant'),
      chat('d', '2026-10-07T10:00:00Z'),
      chat('e', '2026-10-07T11:00:00Z', 'deleted-workspace'),
      chat('f', '2026-10-07T12:00:00Z', 'old'),
      chat('g', '2026-10-02T10:00:00Z', 'thesis', true),
    ],
    workspaces,
  );
  assert.deepEqual(groups.map(g => g.workspace?.id ?? null), ['grant', 'thesis', null]);
  // Pinned chat first inside its workspace, then newest first.
  assert.deepEqual(groups[1].items.map(c => c.id), ['g', 'b', 'a']);
  // A deleted workspace's chat is in "No workspace"; an archived one's is hidden.
  assert.deepEqual(groups[2].items.map(c => c.id), ['e', 'd']);
  assert.ok(!groups.some(g => g.items.some(c => c.id === 'f')));
  assert.deepEqual(groupChatsByWorkspace([], workspaces), []);
});

test('the scope chip says what a question searches', () => {
  const thesis = workspace('thesis', { name: 'Thesis', sourceCounts: counts(1, 2) });
  assert.equal(scopeChip(thesis, false, 9).label, 'Thesis · 3 sources');
  assert.match(scopeChip(thesis, false, 9).title, /only the sources of “Thesis” \(1 folder · 2 files\)/);
  const all = scopeChip(thesis, true, 9);
  assert.equal(all.label, 'All my library · Thesis');
  assert.match(all.title, /instructions and memories of “Thesis” still apply/);
  const empty = scopeChip(workspace('e', { name: 'Empty' }), false, 9);
  assert.equal(empty.label, 'Empty · no sources');
  assert.match(empty.title, /find nothing/);
  assert.equal(scopeChip(null, false, 4).label, 'All sources · 4');
  assert.equal(scopeChip(null, false, 0).label, 'No sources yet');
});

test('a workspace question carries the workspace, never the Library selection', () => {
  const library = { sourceIds: ['src-1', 'src-2'], sourceFiles: [] };
  assert.deepEqual(answerScope(library, null, false), library);
  assert.deepEqual(answerScope(library, 'ws-1', false), { sourceIds: [], sourceFiles: [], workspaceId: 'ws-1' });
  assert.deepEqual(answerScope(null, 'ws-1', true), { sourceIds: [], sourceFiles: [], workspaceId: 'ws-1', searchAll: true });
  // "Ask about this file" in a workspace stays a narrower limit.
  assert.deepEqual(
    answerScope({ sourceIds: [], sourceFiles: ['C:/a.pdf'], pages: [3] }, 'ws-1', false),
    { sourceIds: [], sourceFiles: ['C:/a.pdf'], pages: [3], workspaceId: 'ws-1' },
  );
  assert.equal(answerScope(null, null, true), null);
  assert.equal(answerScope(null, '  ', false), null);
});

test('source summaries count each kind', () => {
  assert.equal(sourceSummary(counts()), 'No sources yet');
  assert.equal(sourceSummary(counts(2, 1, 0, 3)), '2 folders · 1 file · 3 papers');
  assert.equal(sourceSummary(counts(0, 0, 1)), '1 snippet');
});

test('templates list the named ones first and blank last', () => {
  const t = (id: string): WorkspaceTemplate => ({ id, name: id, description: '', icon: 'folder', color: 'neutral', instructions: '' });
  assert.deepEqual(
    orderTemplates([t('blank'), t('literature_review'), t('grant_proposal'), t('paper_writing'), t('client_audit')]).map(x => x.id),
    ['literature_review', 'grant_proposal', 'paper_writing', 'client_audit', 'blank'],
  );
});

test('workspaces sort pinned first, then by latest activity', () => {
  const sorted = sortWorkspaces([
    { ...workspace('a'), lastChatAt: '2026-10-03T00:00:00Z' },
    { ...workspace('b'), lastActiveAt: '2026-10-06T00:00:00Z', lastChatAt: null },
    { ...workspace('c', { pinned: true }), lastChatAt: null },
  ]);
  assert.deepEqual(sorted.map(w => w.id), ['c', 'b', 'a']);
});

test('instruction diffs mark added and removed lines like the backend', () => {
  assert.deepEqual(lineDiff('a\nb\nc', 'a\nc\nd'), [
    { op: 'same', text: 'a' },
    { op: 'removed', text: 'b' },
    { op: 'same', text: 'c' },
    { op: 'added', text: 'd' },
  ]);
  assert.deepEqual(lineDiff('', 'x'), [{ op: 'added', text: 'x' }]);
  assert.deepEqual(lineDiff('x', ''), [{ op: 'removed', text: 'x' }]);
  assert.deepEqual(lineDiff('', ''), []);
  assert.deepEqual(parseDiff([{ op: 'added', text: 'y' }]), [{ op: 'added', text: 'y' }]);
  assert.equal(parseDiff([{ op: 'moved', text: 'y' }]), null);
  assert.equal(parseDiff('nope'), null);
});

test('a new chat starts in the open workspace, else the active chat’s', () => {
  assert.equal(workspaceForNewChat('ws-open', 'ws-chat'), 'ws-open');
  assert.equal(workspaceForNewChat(null, 'ws-chat'), 'ws-chat');
  assert.equal(workspaceForNewChat(null, undefined), null);
  assert.equal(workspaceForNewChat('  ', null), null);
});

test('paths are matched to folders across separators', () => {
  assert.ok(pathWithin('C:\\Docs\\Thesis\\a.pdf', 'c:/docs/thesis', true));
  assert.ok(!pathWithin('C:/Docs/Thesis-old/a.pdf', 'C:/Docs/Thesis', true));
  assert.ok(!pathWithin('/home/a/Docs/x.pdf', '/home/a/docs', false));
  assert.ok(!pathWithin('/x', '', false));
});
