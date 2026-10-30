/**
 * Data kept from an earlier read is never shown as current once a refresh
 * failed. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/freshness.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { STALE_TITLE, freshness } from '../src/lib/freshness.ts';
import { stalePill } from '../src/features/connectors/slackModel.ts';
import type { Pill } from '../src/features/connectors/slackModel.ts';

test('what is on screen and whether the latest read worked decide the state', () => {
  assert.equal(freshness(false, null), 'loading');
  assert.equal(freshness(false, 'backend down'), 'failed');
  assert.equal(freshness(true, 'backend down'), 'stale');
  assert.equal(freshness(true, null), 'fresh');
});

test('a connector kept as "Connected" says it could not refresh once a read failed', () => {
  const connected: Pill = { kind: 'connected', tone: 'ok', label: 'Connected', reason: null, team: 'Lab' };
  const pill = stalePill(connected);
  assert.notEqual(pill.label, 'Connected');
  assert.equal(pill.label, STALE_TITLE);
  assert.notEqual(pill.tone, 'ok', 'not drawn as healthy');
  assert.equal(pill.kind, 'attention');
  assert.equal(pill.reason, 'last known: connected', 'the last known state is still said');
  assert.equal(pill.team, 'Lab');
});
