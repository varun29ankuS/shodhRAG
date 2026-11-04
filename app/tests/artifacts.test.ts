/**
 * The Artifacts tab: where the preview window goes and what it may say, a build's progress
 * line, the finish card's choices, and the navigation that opens a creation. Run with Node
 * 22.6+:
 *   node --experimental-strip-types --test app/tests/artifacts.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  ARTIFACT_PANES,
  CHANGE_DRAFT,
  FLOATING_LAYER_SELECTOR,
  MIN_PREVIEW_HEIGHT,
  PREVIEW_EVENT,
  PREVIEW_NETWORK_OFF_TEXT,
  PREVIEW_NETWORK_ON_TEXT,
  buildProgress,
  fileRows,
  filterCreations,
  finishCard,
  isArtifactPane,
  parsePreviewEvent,
  previewPlacement,
  previousVersion,
  progressLine,
  samePlacement,
  sizeLabel,
  startsBuilding,
} from '../src/features/artifacts/model.ts';
import type { Box, Creation, StepLike } from '../src/features/artifacts/model.ts';
import { VIEW_TABS, VIEW_TAB_LABELS, normalizeViewTab } from '../src/lib/viewTabs.ts';

const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8');

test('agent-built pages never load in the app: no frame, no preview address, no grants', () => {
  // The page shows in a window of its own label that Rust opens (creations/preview_window.rs);
  // the app's own page renders no frame of it and never learns its address.
  const page = read('../src/features/artifacts/ArtifactPage.tsx');
  assert.doesNotMatch(page, /<iframe/);
  assert.doesNotMatch(page, /srcDoc|srcdoc/);
  for (const file of ['ArtifactPage.tsx', 'api.ts', 'model.ts', 'ArtifactsView.tsx', 'CreatorFinishCard.tsx']) {
    assert.doesNotMatch(read(`../src/features/artifacts/${file}`), /shodh-preview/, file);
  }
  const rust = read('../src-tauri/src/creation_commands.rs');
  assert.match(rust, /pub struct PreviewInfo \{\s*\/\/\/[^\n]*\n\s*pub label: String,/);
  assert.doesNotMatch(rust, /pub url: String/);
  // No capability names the preview window's label.
  for (const file of ['main.json', 'print.json']) {
    const capability = JSON.parse(read(`../src-tauri/capabilities/${file}`)) as { webviews: string[] };
    for (const label of capability.webviews) assert.doesNotMatch(label, /^preview/, `${file}: ${label}`);
  }
});

test('the app frames only blobs, in release and in development', () => {
  const config = JSON.parse(read('../src-tauri/tauri.conf.json')) as {
    app: { security: Record<string, Record<string, string>> };
  };
  for (const key of ['csp', 'devCsp']) {
    const frames = config.app.security[key]['frame-src'].split(/\s+/).sort();
    assert.deepEqual(frames, ['blob:'], key);
    for (const [directive, value] of Object.entries(config.app.security[key])) {
      assert.doesNotMatch(value, /shodh-preview/, `${key} ${directive}`);
    }
    // The app's own scripts stay as strict as before: no inline scripts in release.
    if (key === 'csp') assert.doesNotMatch(config.app.security[key]['script-src'], /unsafe-inline/);
  }
});

const box = (left: number, top: number, right: number, bottom: number): Box => ({ left, top, right, bottom });
const viewport = { width: 1400, height: 900 };

test('the preview window covers the visible pane and makes room for what floats over it', () => {
  const pane = box(300, 320, 1200, 860);
  const page = box(240, 48, 1400, 900);
  assert.deepEqual(previewPlacement(pane, page, viewport, []), {
    x: 300, y: 320, width: 900, height: 540, viewportWidth: 1400, viewportHeight: 900,
  });
  // Scrolled: only the part the page shows.
  assert.deepEqual(previewPlacement(box(300, 20, 1200, 600), page, viewport, [])?.y, 48);
  // The dock pill at the bottom right: the preview ends above it, with a gap.
  const pill = { box: box(1100, 844, 1384, 884), modal: false };
  assert.equal(previewPlacement(pane, page, viewport, [pill])?.height, 844 - 8 - 320);
  // A layer elsewhere changes nothing.
  const aside = { box: box(0, 0, 200, 900), modal: false };
  assert.equal(previewPlacement(pane, page, viewport, [aside])?.height, 540);
  // The expanded dock (fixed bottom-4 right-4, 380 wide, 470 tall): too tall to end above,
  // so the preview stays beside it, full height.
  const dock = { box: box(1400 - 16 - 380, 900 - 16 - 470, 1400 - 16, 900 - 16), modal: false };
  assert.deepEqual(previewPlacement(pane, page, viewport, [dock]), {
    x: 300, y: 320, width: 1004 - 8 - 300, height: 540, viewportWidth: 1400, viewportHeight: 900,
  });
  // A popover over its top left: beside it, on the far side.
  assert.equal(previewPlacement(pane, page, viewport, [{ box: box(400, 300, 700, 400), modal: false }])?.x, 708);
  // The dock and a toast above it: clear of both.
  const toast = { box: box(1020, 340, 1384, 404), modal: false };
  assert.equal(previewPlacement(pane, page, viewport, [dock, toast])?.width, 1004 - 8 - 300);
  // A layer across its whole top, a modal dialog anywhere, or too little room: no window.
  assert.equal(previewPlacement(pane, page, viewport, [{ box: box(300, 300, 1200, 400), modal: false }]), null);
  assert.equal(previewPlacement(pane, page, viewport, [{ box: box(0, 0, 10, 10), modal: true }]), null);
  assert.equal(previewPlacement(box(300, 800, 1200, 1300), page, viewport, []), null);
  assert.equal(previewPlacement(box(300, 320, 400, 860), page, viewport, []), null);
  // Always inside the window, in whole pixels (preview_window.rs checks the same).
  const placed = previewPlacement(box(299.6, 320.4, 1500.2, 950.7), page, viewport, []);
  assert.ok(placed);
  assert.ok(placed.x + placed.width <= viewport.width && placed.y + placed.height <= viewport.height);
  assert.ok(Number.isInteger(placed.x) && Number.isInteger(placed.width) && placed.height >= MIN_PREVIEW_HEIGHT);
  assert.ok(samePlacement(placed, { ...placed }));
  assert.equal(samePlacement(placed, { ...placed, x: placed.x + 1 }), false);
  assert.ok(samePlacement(null, null));
  assert.equal(samePlacement(placed, null), false);
  // The layers it looks for: dialogs, menus, lists, tooltips, toasts and the dock pill.
  for (const selector of ['[role="dialog"]', '[role="menu"]', '[role="listbox"]', '[data-sonner-toast]', '[data-floating-layer]']) {
    assert.ok(FLOATING_LAYER_SELECTOR.includes(selector), selector);
  }
  assert.match(read('../src/components/shell/ConversationDock.tsx'), /data-floating-layer=""/);
});

test('only Rust reports on the preview window, in two plain states', () => {
  assert.equal(PREVIEW_EVENT, 'creation-preview');
  assert.match(read('../src-tauri/src/creations/preview_window.rs'), /pub const PREVIEW_EVENT: &str = "creation-preview";/);
  const label = 'preview-0123456789abcdef0123456789abcdef';
  assert.deepEqual(parsePreviewEvent({ label, state: 'loaded' }), { label, state: 'loaded' });
  assert.deepEqual(parsePreviewEvent({ label, state: 'blocked' }), { label, state: 'blocked' });
  for (const bad of [null, 'loaded', { label }, { label, state: 'navigated' }, { label: 7, state: 'loaded' }]) {
    assert.equal(parsePreviewEvent(bad), null, JSON.stringify(bad));
  }
});


const step = (tool: string, status: string, args: unknown = {}, label = tool, detail: unknown = null): StepLike => ({
  tool,
  status,
  args,
  label,
  detail,
});

test('building starts with the first file or command, and the progress line says what is done', () => {
  for (const tool of ['write', 'edit', 'run_command']) assert.ok(startsBuilding(tool), tool);
  for (const tool of ['read', 'grep', 'finish_creation', 'open_artifact']) assert.equal(startsBuilding(tool), false, tool);
  const steps = [
    step('write', 'done', { path: 'index.html' }),
    step('write', 'done', { path: 'app.js' }),
    step('edit', 'done', { path: 'index.html' }),
    step('run_command', 'done'),
    step('write', 'running', { path: 'style.css' }, 'Writing style.css'),
  ];
  const progress = buildProgress(steps);
  assert.deepEqual(progress, { files: 2, commands: 1, now: 'Writing style.css', waiting: false });
  assert.equal(progressLine(progress, true), 'Writing style.css · 2 files written · 1 command run');
  const waiting = buildProgress([step('run_command', 'awaiting_approval', {}, 'Running npm install')]);
  assert.equal(progressLine(waiting, true), 'Waiting for your approval · 0 files written');
  assert.equal(progressLine(buildProgress([]), false), '0 files written');
});

test('the finish card shows the agent\'s summary and choices as plain, capped text', () => {
  const finished = step('finish_creation', 'done', {}, 'Finishing', {
    summary: '  A snake game  ',
    choices: ['Canvas, no libraries', 7, '', '  Arrow keys to steer  ', 'x'.repeat(400)],
  });
  const card = finishCard([step('write', 'done'), finished]);
  assert.deepEqual(card?.summary, 'A snake game');
  assert.deepEqual(card?.choices.slice(0, 2), ['Canvas, no libraries', 'Arrow keys to steer']);
  assert.equal(card?.choices[2].length, 160);
  // Unfinished, failed or empty finishes show no card.
  for (const steps of [
    [step('finish_creation', 'running', {}, 'x', { summary: 'A', choices: [] })],
    [step('finish_creation', 'failed', {}, 'x', { summary: 'A', choices: [] })],
    [step('finish_creation', 'done', {}, 'x', { summary: '  ', choices: [] })],
    [step('finish_creation', 'done', {}, 'x', 'A game')],
    [step('suggest_mode', 'done', {}, 'x', { summary: 'A', choices: [] })],
  ]) {
    assert.equal(finishCard(steps), null, JSON.stringify(steps));
  }
  const many = step('finish_creation', 'done', {}, 'x', { summary: 'A', choices: Array.from({ length: 30 }, (_, i) => `c${i}`) });
  assert.equal(finishCard([many])?.choices.length, 12);
  assert.equal(CHANGE_DRAFT, 'Change: ');
});

test('the Artifacts tab comes right after Ask and the assistant can open its panes', () => {
  assert.equal(VIEW_TABS[0], 'ask');
  assert.equal(VIEW_TABS[1], 'artifacts');
  assert.equal(VIEW_TAB_LABELS.artifacts, 'Artifacts');
  assert.equal(normalizeViewTab('artifacts'), 'artifacts');
  assert.deepEqual([...ARTIFACT_PANES], ['preview', 'files', 'versions']);
  assert.ok(isArtifactPane('files'));
  assert.equal(isArtifactPane('settings'), false);
  // The Rust tool offers the same panes.
  const rust = read('../src-tauri/src/agent_tools/artifacts.rs');
  assert.match(rust, /pub const PANES: \[&str; 3\] = \["preview", "files", "versions"\];/);
  // The Library no longer holds the Generated gallery: the Artifacts tab does.
  assert.doesNotMatch(read('../src/features/library/LibraryView.tsx'), /<VisualGallery/);
  assert.match(read('../src/features/artifacts/ArtifactsView.tsx'), /<VisualGallery conversationId=\{null\} \/>/);
});

const creation = (patch: Partial<Creation>): Creation => ({
  id: 'cr_0123456789abcdef0123456789abcdef',
  conversationId: 'c1',
  title: 'A pomodoro timer',
  folder: 'C:/Users/x/Documents/Shodh Creations/pomodoro-timer',
  createdAt: '2026-11-03T10:00:00Z',
  updatedAt: '2026-11-03T10:00:00Z',
  status: 'ready',
  summary: null,
  choices: [],
  versions: [],
  ...patch,
});

test('creations are searched by title, summary and choices; versions compare with the one before', () => {
  const list = [
    creation({ title: 'A pomodoro timer' }),
    creation({ id: 'cr_1', title: 'Snake', summary: 'A canvas game' }),
    creation({ id: 'cr_2', title: 'Notes', choices: ['Saved in localStorage'] }),
  ];
  assert.deepEqual(filterCreations(list, 'TIMER').map(c => c.title), ['A pomodoro timer']);
  assert.deepEqual(filterCreations(list, 'canvas').map(c => c.title), ['Snake']);
  assert.deepEqual(filterCreations(list, 'localstorage').map(c => c.title), ['Notes']);
  assert.equal(filterCreations(list, '  ').length, 3);
  const versions = [1, 2, 4].map(number => ({ number, createdAt: '', runId: null, files: 1, bytes: 1, leftOut: 0 }));
  assert.equal(previousVersion(versions, 1), 0);
  assert.equal(previousVersion(versions, 4), 2);
});

test('files list as a tree, each folder once before its files', () => {
  const rows = fileRows([
    { path: 'src/app.js', size: 10 },
    { path: 'index.html', size: 2048 },
    { path: 'src/lib/util.js', size: 5 },
  ]);
  assert.deepEqual(
    rows.map(r => `${'  '.repeat(r.depth)}${r.name}${r.folder ? '/' : ''}`),
    ['index.html', 'src/', '  app.js', '  lib/', '    util.js'],
  );
  assert.equal(sizeLabel(2048), '2 KB');
  assert.equal(sizeLabel(12), '12 B');
  assert.equal(sizeLabel(3 * 1024 * 1024), '3.0 MB');
});

test('the preview never claims a sealed network, and says what the switch opens', () => {
  assert.doesNotMatch(PREVIEW_NETWORK_OFF_TEXT, /cannot reach the internet|no network|sealed/i);
  assert.match(PREVIEW_NETWORK_OFF_TEXT, /WebRTC and DNS look-ahead are off/);
  assert.match(PREVIEW_NETWORK_OFF_TEXT, /might still/);
  assert.match(PREVIEW_NETWORK_ON_TEXT, /https/);
  const page = read('../src/features/artifacts/ArtifactPage.tsx');
  assert.match(page, /network \? PREVIEW_NETWORK_ON_TEXT : PREVIEW_NETWORK_OFF_TEXT/);
  // Every preview page and response turn the channels CSP misses off (preview.rs).
  const rust = read('../src-tauri/src/creations/preview.rs');
  assert.match(rust, /\.header\("X-DNS-Prefetch-Control", "off"\)/);
  assert.match(rust, /'RTCPeerConnection'/);
});
