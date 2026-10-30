/**
 * Requests for a view that has not mounted yet expire when the view never
 * shows. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/oneShot.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ONE_SHOT_TTL_MS, OneShot } from '../src/lib/oneShot.ts';
import { peekArchivedRequest, requestArchivedWorkspaces, takeArchivedRequest } from '../src/features/workspaces/archivedRequest.ts';
import { peekRequestedConnectorPage, requestConnectorPage, takeRequestedConnectorPage } from '../src/features/connectors/navigation.ts';

function clock() {
  let now = 1_000;
  return { now: () => now, advance: (ms: number) => { now += ms; } };
}

test('a request is read by every render and taken once', () => {
  const time = clock();
  const request = new OneShot<string>({ now: time.now });
  assert.equal(request.peek(), null);
  request.set('memory');
  assert.equal(request.peek(), 'memory');
  assert.equal(request.peek(), 'memory', 'a render that runs again reads it too');
  assert.equal(request.take(), 'memory');
  assert.equal(request.take(), null);
});

test('a request whose view never showed expires', () => {
  const time = clock();
  const request = new OneShot<string>({ now: time.now });
  request.set('memory');
  time.advance(ONE_SHOT_TTL_MS);
  assert.equal(request.peek(), 'memory', 'still waiting at the limit');
  time.advance(1);
  assert.equal(request.peek(), null, 'gone after it');
  assert.equal(request.take(), null);
});

test('a newer request replaces an older one and starts its own time', () => {
  const time = clock();
  const request = new OneShot<string>({ now: time.now, ttlMs: 100 });
  request.set('search');
  time.advance(90);
  request.set('memory');
  time.advance(90);
  assert.equal(request.take(), 'memory');
});

test("the views' requests expire when their view never shows", t => {
  t.mock.timers.enable({ apis: ['Date'], now: 50_000 });
  const target = { dispatchEvent: () => true };
  takeArchivedRequest();
  takeRequestedConnectorPage();

  requestArchivedWorkspaces(target);
  requestConnectorPage({ connector: 'slack', tab: 'indexing' });
  t.mock.timers.tick(ONE_SHOT_TTL_MS - 1);
  assert.equal(peekArchivedRequest(), true, 'a view switch, even one waiting on code, finds it');
  assert.deepEqual(peekRequestedConnectorPage(), { connector: 'slack', tab: 'indexing' });
  t.mock.timers.tick(2);
  assert.equal(peekArchivedRequest(), false, 'opening Workspaces by hand later does not show the archived list');
  assert.equal(takeRequestedConnectorPage(), null, 'opening Connectors by hand later shows the gallery');
});
