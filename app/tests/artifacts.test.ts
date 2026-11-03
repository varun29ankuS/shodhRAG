/**
 * The Artifacts tab: the preview frame's isolation, a build's progress line, the finish
 * card's choices, and the navigation that opens a creation. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/artifacts.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  ARTIFACT_PANES,
  CHANGE_DRAFT,
  PREVIEW_ALLOW,
  PREVIEW_SANDBOX,
  buildProgress,
  fileRows,
  filterCreations,
  finishCard,
  isArtifactPane,
  isPreviewUrl,
  previousVersion,
  progressLine,
  sandboxIsIsolated,
  sizeLabel,
  startsBuilding,
} from '../src/features/artifacts/model.ts';
import type { Creation, StepLike } from '../src/features/artifacts/model.ts';
import { VIEW_TABS, VIEW_TAB_LABELS, normalizeViewTab } from '../src/lib/viewTabs.ts';

const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8');

test('agent-built pages run in a frame that can only run scripts', () => {
  assert.equal(PREVIEW_SANDBOX, 'allow-scripts');
  assert.ok(sandboxIsIsolated(PREVIEW_SANDBOX));
  for (const loose of ['allow-scripts allow-same-origin', 'allow-popups', 'allow-top-navigation', 'allow-forms', 'allow-scripts allow-modals']) {
    assert.equal(sandboxIsIsolated(loose), false, loose);
  }
  // No powerful feature is delegated to the frame.
  for (const feature of ['camera', 'microphone', 'geolocation', 'clipboard-read', 'clipboard-write', 'display-capture']) {
    assert.match(PREVIEW_ALLOW, new RegExp(`${feature} 'none'`), feature);
  }
  // Every frame in the app's source that shows a creation uses these, and none is same-origin.
  const page = read('../src/features/artifacts/ArtifactPage.tsx');
  assert.match(page, /sandbox=\{PREVIEW_SANDBOX\}/);
  assert.match(page, /allow=\{PREVIEW_ALLOW\}/);
  assert.match(page, /referrerPolicy="no-referrer"/);
  assert.doesNotMatch(page, /allow-same-origin/);
  assert.doesNotMatch(page, /srcDoc|srcdoc/, 'a srcdoc frame would inherit the app\'s policy and origin rules');
});

test('only previews of the app\'s own scheme count as previews', () => {
  const token = '0123456789abcdef0123456789abcdef';
  assert.ok(isPreviewUrl(`shodh-preview://localhost/${token}/index.html`));
  assert.ok(isPreviewUrl(`http://shodh-preview.localhost/${token}/index.html`));
  for (const url of [
    `https://shodh-preview.localhost/${token}/index.html`,
    `http://ipc.localhost/${token}/`,
    `http://shodh-preview.localhost.evil.example/${token}/`,
    `http://tauri.localhost/${token}/`,
    'http://shodh-preview.localhost/short/index.html',
  ]) {
    assert.equal(isPreviewUrl(url), false, url);
  }
});

test('the app lets frames load only blobs and previews, in release and in development', () => {
  const config = JSON.parse(read('../src-tauri/tauri.conf.json')) as {
    app: { security: Record<string, Record<string, string>> };
  };
  for (const key of ['csp', 'devCsp']) {
    const frames = config.app.security[key]['frame-src'].split(/\s+/).sort();
    assert.deepEqual(frames, ['blob:', 'http://shodh-preview.localhost', 'shodh-preview:'], key);
    // The app's own scripts stay as strict as before: no inline scripts in release.
    if (key === 'csp') assert.doesNotMatch(config.app.security[key]['script-src'], /unsafe-inline/);
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
