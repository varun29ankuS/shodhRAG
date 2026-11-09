/**
 * Recall's words and choices (src/features/recall/recallModel.ts) and its status strip run
 * (src/features/status/systemStatus.ts).
 *   node --experimental-strip-types --test app/tests/recall.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  agoText,
  dayOf,
  includeUndecided,
  isReceipt,
  parsePatterns,
  recallStage,
  recallStatusLine,
  roleLabel,
  timeOf,
  waitingLine,
  withProject,
  withTool,
} from '../src/features/recall/recallModel.ts';
import type { RecallSettings, RecallStatus } from '../src/features/recall/api.ts';
import { readSystemStatus, recallRunText } from '../src/features/status/systemStatus.ts';

const NOW = new Date('2026-11-09T12:00:00Z');

function settings(extra: Partial<RecallSettings> = {}): RecallSettings {
  return { enabled: false, tools: [], projects: {}, patterns: [], idleMinutes: 10, ...extra };
}

function status(extra: Partial<RecallStatus> = {}): RecallStatus {
  return {
    settings: settings(),
    tools: [],
    projects: [],
    indexedSessions: 0,
    indexedTurns: 0,
    secretsRedacted: 0,
    unreadableLines: 0,
    running: null,
    lastSync: null,
    error: null,
    searchModels: true,
    ...extra,
  };
}

test('the card walks from off to tools to projects to ready', () => {
  assert.equal(recallStage(status()), 'off');
  assert.equal(recallStage(status({ settings: settings({ enabled: true }) })), 'tools');
  assert.equal(recallStage(status({ settings: settings({ enabled: true, tools: ['codex'] }) })), 'projects');
  assert.equal(
    recallStage(status({ settings: settings({ enabled: true, tools: ['codex'], projects: { '/a': false } }) })),
    'projects',
    'an excluded project reads nothing',
  );
  assert.equal(recallStage(status({ settings: settings({ enabled: true, tools: ['codex'], projects: { '/a': true } }) })), 'ready');
});

test('choices change only what they name', () => {
  const s = settings({ tools: ['codex'], projects: { '/a': false } });
  assert.deepEqual(withTool(s, 'claude_code', true).tools, ['claude_code', 'codex']);
  assert.deepEqual(withTool(s, 'codex', false).tools, []);
  assert.deepEqual(withProject(s, '/b', true).projects, { '/a': false, '/b': true });
  assert.deepEqual(includeUndecided(s, ['/a', '/b', '/c']).projects, { '/a': false, '/b': true, '/c': true }, 'an exclusion stays');
  assert.deepEqual(s.projects, { '/a': false }, 'the settings given are not changed');
});

test('patterns are one a line', () => {
  assert.deepEqual(parsePatterns('ACME-[0-9]{6}\r\n\n  internal\\.example\\.com  \n'), ['ACME-[0-9]{6}', 'internal\\.example\\.com']);
  assert.deepEqual(parsePatterns('   \n'), []);
});

test('the status line says what runs and what is indexed', () => {
  assert.equal(
    recallStatusLine(status({ indexedSessions: 128, secretsRedacted: 4, lastSync: { at: '2026-11-09T11:57:00Z', indexed: 1, removed: 0, failed: 0, waiting: 2, stopped: null } }), NOW),
    '128 sessions indexed · 4 secrets redacted · checked 3 minutes ago',
  );
  assert.equal(recallStatusLine(status({ indexedSessions: 1 }), NOW), '1 session indexed');
  assert.equal(
    recallStatusLine(status({ running: { sourceId: 'recall', done: 3, total: 40, current: 'lumen-cache', paused: false } }), NOW),
    'Indexing 3 of 40 sessions (lumen-cache)',
  );
  assert.equal(
    recallStatusLine(status({ running: { sourceId: 'recall', done: 3, total: 40, current: null, paused: true } }), NOW),
    'Paused: 3 of 40 sessions indexed',
  );
  assert.equal(recallStatusLine(status({ running: { sourceId: 'recall', done: 0, total: 0, current: null, paused: false } }), NOW), 'Looking for new sessions');
  assert.equal(
    waitingLine(status({ lastSync: { at: '2026-11-09T11:57:00Z', indexed: 0, removed: 0, failed: 0, waiting: 1, stopped: null } })),
    '1 session still in use: indexed once idle for 10 minutes',
  );
  assert.equal(waitingLine(status()), null);
});

test('times, roles and receipts read plainly', () => {
  assert.equal(agoText('2026-11-09T11:59:50Z', NOW), 'just now');
  assert.equal(agoText('2026-11-09T10:00:00Z', NOW), '2 hours ago');
  assert.equal(agoText('2026-11-07T12:00:00Z', NOW), '2 days ago');
  assert.equal(agoText('nonsense', NOW), null);
  assert.equal(dayOf('2026-10-12T09:00:00Z'), '2026-10-12');
  assert.equal(timeOf('2026-10-12T09:03:00Z'), '2026-10-12, 09:03');
  assert.equal(timeOf(null), null);
  assert.equal(roleLabel('tool_output', 'Bash'), 'Output of Bash');
  assert.equal(roleLabel('tool_call', null), 'Tool call');
  assert.equal(roleLabel('user', null), 'You');
  assert.ok(isReceipt('12:40'));
  assert.ok(!isReceipt('12-40'));
  assert.ok(!isReceipt(null));
});

test('the status strip reads Recall\'s run', () => {
  const read = readSystemStatus({ recall: { sourceId: 'recall', done: 2, total: 5, current: 'lumen-cache', paused: false } });
  assert.deepEqual(read?.recall, { sourceId: 'recall', done: 2, total: 5, current: 'lumen-cache' });
  assert.equal(readSystemStatus({})?.recall, null, 'an older backend sends none');
  assert.equal(readSystemStatus({ recall: { done: 1 } })?.recall, null);
  assert.equal(recallRunText({ sourceId: 'recall', done: 2, total: 5, current: 'lumen-cache' }), '2 of 5 sessions (lumen-cache)');
  assert.equal(recallRunText({ sourceId: 'recall', done: 0, total: 0, current: null }), 'Looking for new sessions');
});
