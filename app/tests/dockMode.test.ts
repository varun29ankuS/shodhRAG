/**
 * Conversation dock layout tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/dockMode.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  DOCK_DEFAULT_WIDTH,
  DOCK_KEY_STEP,
  DOCK_KEY_STEP_LARGE,
  DOCK_MIN_WIDTH,
  DOCK_PAGE_RESERVE,
  DOCK_PANEL_MIN_WINDOW,
  clampDockWidth,
  defaultDockRequest,
  defaultViewPrefs,
  maxDockWidth,
  panelFits,
  parseDockPrefs,
  resolveDockLayout,
  serializeDockPrefs,
  viewPrefs,
  widthForDrag,
  widthForKey,
  withViewPrefs,
} from '../src/lib/dockMode.ts';
import { VIEW_TABS } from '../src/lib/viewTabs.ts';

test('every view starts with the dock minimised to a pill', () => {
  for (const view of VIEW_TABS) {
    assert.equal(defaultDockRequest(view), 'pill');
    assert.deepEqual(defaultViewPrefs(view), { pinned: false, width: DOCK_DEFAULT_WIDTH });
  }
});

test('a closed dock is a pill at any window width', () => {
  for (const w of [400, DOCK_PANEL_MIN_WINDOW, 2560]) {
    assert.equal(resolveDockLayout(false, w, true), 'pill');
    assert.equal(resolveDockLayout(false, w, false), 'pill');
  }
});

test('an open dock is a side panel when it fits and an overlay just below the threshold', () => {
  assert.equal(resolveDockLayout(true, DOCK_PANEL_MIN_WINDOW, true), 'panel');
  assert.equal(resolveDockLayout(true, DOCK_PANEL_MIN_WINDOW + 1, true), 'panel');
  assert.equal(resolveDockLayout(true, DOCK_PANEL_MIN_WINDOW - 1, true), 'overlay');
  assert.equal(resolveDockLayout(true, 1920, true), 'panel');
  assert.equal(resolveDockLayout(true, 600, true), 'overlay');
  assert.equal(resolveDockLayout(true, Number.NaN, true), 'overlay');
});

test('a pinned panel restored on a narrow window stays a pill instead of covering the page', () => {
  assert.equal(resolveDockLayout(true, 1920, false), 'panel');
  assert.equal(resolveDockLayout(true, DOCK_PANEL_MIN_WINDOW, false), 'panel');
  assert.equal(resolveDockLayout(true, DOCK_PANEL_MIN_WINDOW - 1, false), 'pill');
  assert.equal(resolveDockLayout(true, 600, false), 'pill');
});

test('the panel threshold leaves the page reserve next to a minimum-width panel', () => {
  assert.equal(DOCK_PANEL_MIN_WINDOW, DOCK_MIN_WIDTH + DOCK_PAGE_RESERVE);
  assert.equal(panelFits(DOCK_PANEL_MIN_WINDOW), true);
  assert.equal(panelFits(DOCK_PANEL_MIN_WINDOW - 1), false);
  assert.equal(panelFits(Number.POSITIVE_INFINITY), false);
});

test('max width is half the window on wide screens and never below the minimum', () => {
  assert.equal(maxDockWidth(2000), 1000);
  assert.equal(maxDockWidth(1200), 1200 - DOCK_PAGE_RESERVE);
  assert.equal(maxDockWidth(500), DOCK_MIN_WIDTH);
  assert.equal(maxDockWidth(0), DOCK_MIN_WIDTH);
  assert.equal(maxDockWidth(Number.NaN), DOCK_MIN_WIDTH);
  for (const w of [0, 300, 800, 1048, 1280, 1920, 3840]) {
    assert.ok(maxDockWidth(w) >= DOCK_MIN_WIDTH, `max >= min at ${w}`);
  }
});

test('clamp keeps widths within [min, max] and repairs non-finite input', () => {
  assert.equal(clampDockWidth(500, 1920), 500);
  assert.equal(clampDockWidth(100, 1920), DOCK_MIN_WIDTH);
  assert.equal(clampDockWidth(-50, 1920), DOCK_MIN_WIDTH);
  assert.equal(clampDockWidth(5000, 1920), 960);
  assert.equal(clampDockWidth(Number.NaN, 1920), DOCK_DEFAULT_WIDTH);
  assert.equal(clampDockWidth(Number.POSITIVE_INFINITY, 1920), DOCK_DEFAULT_WIDTH);
  assert.equal(clampDockWidth(700, 400), DOCK_MIN_WIDTH);
  assert.equal(clampDockWidth(400.6, 1920), 401);
});

test('arrow keys resize from the left edge; Home and End jump to the bounds', () => {
  assert.equal(widthForKey('ArrowLeft', 400, 1920, false), 400 + DOCK_KEY_STEP);
  assert.equal(widthForKey('ArrowRight', 400, 1920, false), 400 - DOCK_KEY_STEP);
  assert.equal(widthForKey('ArrowLeft', 400, 1920, true), 400 + DOCK_KEY_STEP_LARGE);
  assert.equal(widthForKey('ArrowRight', DOCK_MIN_WIDTH, 1920, false), DOCK_MIN_WIDTH);
  assert.equal(widthForKey('ArrowLeft', 960, 1920, false), 960);
  assert.equal(widthForKey('Home', 600, 1920, false), DOCK_MIN_WIDTH);
  assert.equal(widthForKey('End', 400, 1920, false), 960);
  assert.equal(widthForKey('Enter', 400, 1920, false), null);
  assert.equal(widthForKey('ArrowUp', 400, 1920, false), null);
});

test('dragging the handle left widens and right narrows, clamped', () => {
  assert.equal(widthForDrag(400, 1000, 900, 1920), 500);
  assert.equal(widthForDrag(400, 1000, 1050, 1920), 350);
  assert.equal(widthForDrag(400, 1000, 1500, 1920), DOCK_MIN_WIDTH);
  assert.equal(widthForDrag(400, 1000, 0, 1920), 960);
});

test('parse returns nothing for missing, corrupt or foreign records', () => {
  assert.deepEqual(parseDockPrefs(null), {});
  assert.deepEqual(parseDockPrefs(''), {});
  assert.deepEqual(parseDockPrefs('{not json'), {});
  assert.deepEqual(parseDockPrefs('"open"'), {});
  assert.deepEqual(parseDockPrefs('null'), {});
  assert.deepEqual(parseDockPrefs('[]'), {});
  assert.deepEqual(parseDockPrefs(JSON.stringify({ version: 1, views: { library: { pinned: true, width: 400 } } })), {});
  assert.deepEqual(parseDockPrefs(JSON.stringify({ version: 2, views: [] })), {});
  assert.deepEqual(parseDockPrefs(JSON.stringify({ version: 2 })), {});
});

test('parse drops invalid entries one by one and keeps the valid ones', () => {
  const raw = JSON.stringify({
    version: 2,
    views: {
      library: { pinned: true, width: 420 },
      tasks: { pinned: 'yes', width: 420 },
      activity: { pinned: false, width: 10 },
      settings: { pinned: true, width: 1e9 },
      ask: { pinned: true, width: 400 },
      graph: { pinned: true, width: 400 },
    },
  });
  assert.deepEqual(parseDockPrefs(raw), { library: { pinned: true, width: 420 } });
  assert.deepEqual(
    parseDockPrefs(JSON.stringify({ version: 2, views: { tasks: { pinned: false, width: 512.4 } } })),
    { tasks: { pinned: false, width: 512 } },
  );
  assert.deepEqual(parseDockPrefs(JSON.stringify({ version: 2, views: { tasks: { pinned: true } } })), {});
  assert.deepEqual(parseDockPrefs(JSON.stringify({ version: 2, views: { tasks: null } })), {});
});

test('serialize and parse round-trip, skipping Ask', () => {
  let prefs = withViewPrefs({}, 'library', { pinned: true, width: 450 });
  prefs = withViewPrefs(prefs, 'settings', { width: 333 });
  prefs = withViewPrefs(prefs, 'ask', { pinned: true });
  const parsed = parseDockPrefs(serializeDockPrefs(prefs));
  assert.deepEqual(parsed, {
    library: { pinned: true, width: 450 },
    settings: { pinned: false, width: 333 },
  });
});

test('viewPrefs falls back to the default and withViewPrefs does not mutate', () => {
  const base = withViewPrefs({}, 'tasks', { pinned: true });
  const next = withViewPrefs(base, 'tasks', { width: 500 });
  assert.deepEqual(viewPrefs(base, 'tasks'), { pinned: true, width: DOCK_DEFAULT_WIDTH });
  assert.deepEqual(viewPrefs(next, 'tasks'), { pinned: true, width: 500 });
  assert.deepEqual(viewPrefs(next, 'activity'), defaultViewPrefs('activity'));
});
