/**
 * Loading the lazily loaded views' code ahead of their first visit. Run with
 * Node 22.6+:
 *   node --experimental-strip-types --test app/tests/viewPreload.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { ViewPreloader, settledThenable } from '../src/lib/viewPreload.ts';
import type { IdleScheduler } from '../src/lib/viewPreload.ts';
import { VIEW_TABS } from '../src/lib/viewTabs.ts';

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

test('every lazily loaded view is preloaded, and the shell renders them from the preloaded module', () => {
  const views = readFileSync(new URL('../src/components/shell/views.ts', import.meta.url), 'utf8');
  const loaders = views.slice(views.indexOf('const loaders = {'), views.indexOf('} satisfies'));
  for (const tab of VIEW_TABS.filter(t => t !== 'ask')) {
    assert.match(loaders, new RegExp(`\n  ${tab}: \(\) => import\(`), `${tab} has a loader`);
    assert.match(views, new RegExp(`'${tab}'`), `${tab} is in the idle order`);
  }
  const app = readFileSync(new URL('../src/App-SplitView.tsx', import.meta.url), 'utf8');
  assert.doesNotMatch(app, /lazy\(\(\) => import\('\.\/features\/(tasks\/TasksView|activity|workspaces|library|connectors)/);
  assert.doesNotMatch(app, /lazy\(\(\) => import\('\.\/components\/shell\/SettingsView'\)/);
  assert.match(app, /useEffect\(\(\) => preloadViewsWhenIdle\(\), \[\]\);/);
  const sidebar = readFileSync(new URL('../src/components/shell/Sidebar.tsx', import.meta.url), 'utf8');
  assert.match(sidebar, /onPointerEnter=\{\(\) => preloadView\(view\)\}/);
  assert.match(sidebar, /onFocus=\{\(\) => preloadView\(view\)\}/);
});
