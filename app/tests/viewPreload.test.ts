/**
 * Loading the lazily loaded views' code ahead of their first visit. Run with
 * Node 22.6+:
 *   node --experimental-strip-types --test app/tests/viewPreload.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { PreloadGate, ViewPreloader, settledThenable } from '../src/lib/viewPreload.ts';
import type { GateTimers, IdleScheduler } from '../src/lib/viewPreload.ts';

const flush = () => new Promise<void>(resolve => setImmediate(resolve));

/** Idle callbacks run only when the test says the app is idle. */
function manualIdle() {
  let nextId = 1;
  const queued = new Map<number, () => void>();
  const idle: IdleScheduler = {
    request(callback) {
      const id = nextId++;
      queued.set(id, callback);
      return id;
    },
    cancel(handle) {
      queued.delete(handle as number);
    },
  };
  const runIdle = () => {
    const callbacks = [...queued.values()];
    queued.clear();
    for (const callback of callbacks) callback();
  };
  return { idle, runIdle, queuedCount: () => queued.size };
}

function countingLoaders() {
  const calls: string[] = [];
  const failNext = new Set<string>();
  const loader = (key: string) => () => {
    calls.push(key);
    if (failNext.delete(key)) return Promise.reject(new Error(`chunk ${key} failed`));
    return Promise.resolve({ default: key });
  };
  return {
    calls,
    failNext,
    loaders: { library: loader('library'), tasks: loader('tasks'), settings: loader('settings') },
  };
}

test('concurrent preloads of one view share one load, and a loaded view is not loaded again', async () => {
  const { calls, loaders } = countingLoaders();
  const preloader = new ViewPreloader(loaders);
  assert.equal(preloader.isLoaded('library'), false);
  const first = preloader.load('library');
  const second = preloader.load('library');
  assert.equal(first, second);
  await Promise.all([first, second, preloader.preload('library')]);
  assert.equal(preloader.isLoaded('library'), true);
  await preloader.preload('library');
  assert.deepEqual(calls, ['library']);
});

test('a failed preload never rejects and is tried again next time', async () => {
  const { calls, failNext, loaders } = countingLoaders();
  const preloader = new ViewPreloader(loaders);
  failNext.add('tasks');
  await preloader.preload('tasks');
  assert.equal(preloader.isLoaded('tasks'), false);
  await preloader.preload('tasks');
  assert.equal(preloader.isLoaded('tasks'), true);
  assert.deepEqual(calls, ['tasks', 'tasks']);
});

test("the view's own load reports a failure, then succeeds on the next try with the module", async () => {
  const { failNext, loaders } = countingLoaders();
  const preloader = new ViewPreloader(loaders);
  failNext.add('settings');
  await assert.rejects(preloader.load('settings'), /chunk settings failed/);
  assert.equal(preloader.loaded('settings'), undefined);
  assert.deepEqual(await preloader.load('settings'), { default: 'settings' });
  assert.deepEqual(preloader.loaded('settings'), { default: 'settings' });
});

test('a preloaded module is handed to React.lazy synchronously', () => {
  const module = { default: 'LibraryView' };
  let seen: unknown = null;
  settledThenable(module).then(value => {
    seen = value;
  });
  // Read before any microtask runs: React.lazy's first render reads it the same way.
  assert.equal(seen, module);
  const views = readFileSync(new URL('../src/components/shell/views.ts', import.meta.url), 'utf8');
  assert.match(views, /if \(preloader\.isLoaded\(key\)\) return settledThenable\(/);
});

test('idle preloading loads one view per idle period, in order, skipping loaded views', async () => {
  const { calls, loaders } = countingLoaders();
  const preloader = new ViewPreloader(loaders);
  await preloader.preload('tasks');
  const { idle, runIdle, queuedCount } = manualIdle();
  preloader.preloadWhenIdle(['library', 'tasks', 'settings'], idle);
  assert.deepEqual(calls, ['tasks'], 'nothing loads before the app is idle');
  runIdle();
  await flush();
  assert.deepEqual(calls, ['tasks', 'library']);
  assert.equal(queuedCount(), 1, 'the next view waits for the next idle period');
  runIdle();
  await flush();
  assert.deepEqual(calls, ['tasks', 'library', 'settings'], 'tasks was loaded already');
  runIdle();
  await flush();
  assert.equal(queuedCount(), 0, 'done once every view is loaded');
  assert.deepEqual(calls, ['tasks', 'library', 'settings']);
});

test('stopping idle preloading cancels the waiting callback and loads nothing more', async () => {
  const { calls, loaders } = countingLoaders();
  const preloader = new ViewPreloader(loaders);
  const { idle, runIdle, queuedCount } = manualIdle();
  const stop = preloader.preloadWhenIdle(['library', 'tasks'], idle);
  runIdle();
  await flush();
  assert.deepEqual(calls, ['library']);
  stop();
  assert.equal(queuedCount(), 0);
  runIdle();
  await flush();
  assert.deepEqual(calls, ['library']);
});

test('stopping while a view is loading requests no further idle period', async () => {
  let finish: () => void = () => {};
  const preloader = new ViewPreloader({
    library: () => new Promise<void>(resolve => { finish = resolve; }),
    tasks: () => Promise.resolve(),
  });
  const { idle, runIdle, queuedCount } = manualIdle();
  const stop = preloader.preloadWhenIdle(['library', 'tasks'], idle);
  runIdle();
  await flush(); // the library load has started and is waiting
  stop();
  finish();
  await flush();
  assert.equal(preloader.isLoaded('library'), true, 'the load in progress still completes');
  assert.equal(queuedCount(), 0, 'no idle period is requested after stopping');
  runIdle();
  await flush();
  assert.equal(preloader.isLoaded('tasks'), false);
});

/** Timers that fire only when the test advances the clock. */
function manualTimers() {
  let now = 0;
  let nextId = 1;
  const pending = new Map<number, { due: number; fn: () => void }>();
  const timers: GateTimers = {
    setTimeout(fn, ms) {
      const id = nextId++;
      pending.set(id, { due: now + ms, fn });
      return id;
    },
    clearTimeout(handle) {
      pending.delete(handle as number);
    },
  };
  const advance = (ms: number) => {
    now += ms;
    for (const [id, t] of [...pending]) {
      if (t.due <= now) {
        pending.delete(id);
        t.fn();
      }
    }
  };
  return { timers, advance, pendingCount: () => pending.size };
}

/** Counts starts and stops of the idle fetching. */
function fetching() {
  const log: string[] = [];
  const start = () => {
    log.push('start');
    return () => log.push('stop');
  };
  return { log, start };
}

test('nothing is fetched at launch; the fetching starts after the delay', () => {
  const { timers, advance } = manualTimers();
  const { log, start } = fetching();
  new PreloadGate(start, timers, 25_000);
  advance(24_999);
  assert.deepEqual(log, []);
  advance(1);
  assert.deepEqual(log, ['start']);
});

test('the first finished answer starts it early, once', () => {
  const { timers, advance, pendingCount } = manualTimers();
  const { log, start } = fetching();
  const gate = new PreloadGate(start, timers, 25_000);
  gate.setStreaming(true);
  assert.deepEqual(log, [], 'not while the first answer streams');
  gate.setStreaming(false);
  assert.deepEqual(log, ['start']);
  assert.equal(pendingCount(), 0, 'the delay is no longer needed');
  advance(30_000);
  assert.deepEqual(log, ['start']);
});

test('never while an answer streams: due during one, it waits; it stops when one starts and resumes after', () => {
  const { timers, advance } = manualTimers();
  const { log, start } = fetching();
  const gate = new PreloadGate(start, timers, 25_000);
  gate.setStreaming(true);
  advance(25_000);
  assert.deepEqual(log, [], 'due mid-answer: not started');
  gate.setStreaming(false);
  assert.deepEqual(log, ['start']);
  gate.setStreaming(true);
  assert.deepEqual(log, ['start', 'stop']);
  gate.setStreaming(false);
  assert.deepEqual(log, ['start', 'stop', 'start']);
});

test('disposing stops everything', () => {
  const { timers, advance, pendingCount } = manualTimers();
  const { log, start } = fetching();
  const gate = new PreloadGate(start, timers, 25_000);
  gate.dispose();
  assert.equal(pendingCount(), 0);
  advance(25_000);
  gate.setStreaming(true);
  gate.setStreaming(false);
  assert.deepEqual(log, []);
});

test('only the likely views are fetched while idle, and an idle period is never forced', () => {
  const views = readFileSync(new URL('../src/components/shell/views.ts', import.meta.url), 'utf8');
  assert.match(views, /IDLE_PRELOAD_VIEWS: readonly LazyTab\[\] = \['library', 'workspaces'\];/);
  assert.doesNotMatch(views, /requestIdleCallback\([^)]*timeout/, 'no timeout: a busy app is never interrupted');
});
