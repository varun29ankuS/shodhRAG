/**
 * The Artifacts tab: the preview window and its card, the page guard, a build's progress
 * line, the finish card's choices, and the navigation that opens a creation. Run with Node
 * 22.6+:
 *   node --experimental-strip-types --test app/tests/artifacts.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import {
  ARTIFACT_PANES,
  CHANGE_DRAFT,
  PREVIEW_END_TEXT,
  PREVIEW_EVENT,
  PREVIEW_NETWORK_OFF_TEXT_MAC,
  PREVIEW_NETWORK_OFF_TEXT_WINDOWS,
  PREVIEW_NETWORK_ON_TEXT,
  buildProgress,
  fileRows,
  filterCreations,
  finishCard,
  hasEntryPage,
  isArtifactPane,
  parsePreviewEvent,
  parsePreviewShown,
  previewEndFor,
  previewLoadAfter,
  previewLoadAtEmbed,
  previewNetworkText,
  previewShowingText,
  previewStateLine,
  selectionFor,
  previousVersion,
  progressLine,
  sizeLabel,
  startsBuilding,
} from '../src/features/artifacts/model.ts';
import type { Creation, StepLike } from '../src/features/artifacts/model.ts';
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
  assert.match(rust, /pub struct PreviewInfo \{(?:\s*\/\/\/[^\n]*)*\s*pub label: String,/);
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


test('the page shows inside Artifacts, placed by its pane, and pops out to a window of its own', () => {
  const page = read('../src/features/artifacts/ArtifactPage.tsx');
  const embedded = read('../src/features/artifacts/EmbeddedPreview.tsx');
  // The page itself never measures or frames anything: its preview pane holds EmbeddedPreview.
  for (const gone of ['getBoundingClientRect', 'placePreview', '<iframe']) {
    assert.doesNotMatch(page, new RegExp(gone), gone);
  }
  assert.match(page, /<EmbeddedPreview\b/);
  // EmbeddedPreview reports its box once a frame, and hides the page when it goes.
  assert.match(embedded, /window\.requestAnimationFrame\(report\)/);
  assert.match(embedded, /coveredBy\(pane, overlays, modalOpen, dockCorner\(\)\)/);
  assert.match(embedded, /layerVisible\(\{/);
  assert.doesNotMatch(embedded, /<iframe/);
  // Its toolbar: Reload, Pop out; popped out: Bring to front and Show here.
  for (const label of ['Reload', 'Pop out', 'Bring to front', 'Show here']) {
    assert.ok(embedded.includes(`${label}\n`) || embedded.includes(`'${label}'`), label);
  }
  assert.match(embedded, /role="status" aria-live="polite"/);
  // A build starting opens the pane, so the page shows: its request names no pane.
  assert.match(read('../src/features/artifacts/ArtifactsView.tsx'), /setSelection\(prev => selectionFor\(target, prev\)\)/);
  assert.match(read('../src/features/ask/ChatSessionContext.tsx'), /artifactId: live\.creationId, version: null, pane: null \}/);
  // The popped-out window is a normal one: titled, resizable, decorated, its own close keys honoured.
  const rust = read('../src-tauri/src/creations/preview_window.rs');
  for (const part of ['.decorations(true)', '.resizable(true)', '.title(window_title(']) assert.ok(rust.includes(part), part);
  assert.doesNotMatch(rust, /prevent_close\(|skip_taskbar\(true\)|\.parent\(/);
});

test('a build start opens the page itself, not just the pane', () => {
  const build = selectionFor({ artifactId: 'cr_a', version: null, pane: null }, null);
  assert.deepEqual(build, { id: 'cr_a', version: null, pane: 'preview', showPreview: true, seq: 1 });
  assert.equal(selectionFor({ artifactId: 'cr_a', version: 2, pane: 'preview' }, build).showPreview, true);
  assert.equal(selectionFor({ artifactId: 'cr_a', version: 2, pane: 'preview' }, build).seq, 2);
  // Another pane opens that pane only.
  const files = selectionFor({ artifactId: 'cr_a', version: null, pane: 'files' }, build);
  assert.equal(files.pane, 'files');
  assert.equal(files.showPreview, false);
});

test('the embedded preview reports hidden when it unmounts', () => {
  const embedded = read('../src/features/artifacts/EmbeddedPreview.tsx');
  assert.match(embedded, /creationsApi\.placePreview\(label, \{ rect: null, covered: true \}\)/);
  assert.match(embedded, /dockLayer\.set\(false\);\s*creationsApi\.placePreview\(label, \{ rect: null, covered: true \}\)/);
});

test('pop out leaves a way back, and the pane says where the page is', () => {
  assert.equal(previewShowingText(null, 'embedded'), 'Now · live');
  assert.equal(previewShowingText(3, 'embedded'), 'Version 3 · live');
  assert.equal(previewShowingText(null, 'window'), 'Now · in its own window');
  assert.equal(previewShowingText(3), 'Version 3 · in its own window');
  const embedded = read('../src/features/artifacts/EmbeddedPreview.tsx');
  // Popped out, the pane offers Show here, which brings the page back.
  assert.match(embedded, /\{poppedOut \? \([\s\S]*?onClick=\{\(\) => void showHere\(\)\}[\s\S]*?Show here/);
  const shown = { label: 'preview-0123456789abcdef0123456789abcdef', creationId: 'cr_a', version: 2, network: false, place: 'window' as const };
  assert.equal(previewEndFor(shown, 'cr_a', 2, false), null);
  assert.equal(previewEndFor(shown, 'cr_a', null, false), 'version');
  assert.equal(previewEndFor(shown, 'cr_a', 2, true), 'network');
  // Another creation's page leaves the window alone.
  assert.equal(previewEndFor(shown, 'cr_b', null, true), null);
  for (const why of ['closed', 'stopped', 'version', 'network'] as const) {
    assert.ok(PREVIEW_END_TEXT[why].length > 0, why);
  }
  assert.match(PREVIEW_END_TEXT.version, /another version/);
  assert.match(PREVIEW_END_TEXT.network, /internet switch/);
  assert.deepEqual(parsePreviewShown(shown), shown);
  assert.deepEqual(parsePreviewShown({ ...shown, place: 'embedded' }), { ...shown, place: 'embedded' });
  assert.deepEqual(parsePreviewShown({ ...shown, version: null }), { ...shown, version: null });
  for (const bad of [null, 'x', { ...shown, label: 7 }, { ...shown, network: 'no' }]) {
    assert.equal(parsePreviewShown(bad), null, JSON.stringify(bad));
  }
});

test('only Rust reports on the preview, in four plain states, saying where it is', () => {
  assert.equal(PREVIEW_EVENT, 'creation-preview');
  assert.match(read('../src-tauri/src/creations/preview_window.rs'), /pub const PREVIEW_EVENT: &str = "creation-preview";/);
  const label = 'preview-0123456789abcdef0123456789abcdef';
  for (const state of ['loaded', 'blocked', 'closed', 'stopped'] as const) {
    for (const place of ['window', 'embedded'] as const) {
      assert.deepEqual(parsePreviewEvent({ label, state, place }), { label, state, place });
    }
  }
  for (const bad of [null, 'loaded', { label }, { label, state: 'navigated' }, { label: 7, state: 'loaded' }]) {
    assert.equal(parsePreviewEvent(bad), null, JSON.stringify(bad));
  }
});

/** The page guard as preview.rs ships it (its raw string). */
function guardScript(): string {
  const rust = read('../src-tauri/src/creations/preview.rs');
  const start = rust.indexOf('r#"', rust.indexOf('macro_rules! guard_script'));
  const end = rust.indexOf('"#', start + 3);
  assert.ok(start > 0 && end > start, 'the guard script in preview.rs');
  return rust.slice(start + 3, end);
}

/**
 * A browser window, as far as the guard reaches into it: the WebRTC interfaces, media
 * capture, frames the page makes, and the DOM calls that insert them.
 */
function fakeWindow(): Record<string, unknown> {
  const frames: Record<string, unknown>[] = [];
  const win: Record<string, unknown> = {};
  class Navigator {
    get mediaDevices() { return { getUserMedia: () => 'camera' }; }
    getUserMedia() { return 'camera'; }
  }
  class MediaDevices {
    getUserMedia() { return 'camera'; }
    getDisplayMedia() { return 'screen'; }
  }
  class HTMLIFrameElement {
    child: Record<string, unknown>;
    constructor() { this.child = fakeChild(); }
    get contentWindow() { return this.child; }
    get contentDocument() { return { defaultView: this.child }; }
  }
  class Node {
    appendChild(node: unknown) {
      if (node instanceof HTMLIFrameElement) frames.push(node.child);
      return node;
    }
  }
  class Element extends Node {}
  Object.assign(win, {
    RTCPeerConnection: function RTCPeerConnection() {},
    RTCDataChannel: function RTCDataChannel() {},
    navigator: new Navigator(),
    Navigator,
    MediaDevices,
    HTMLIFrameElement,
    Node,
    Element,
    frames: { get length() { return frames.length; } },
    document: {},
  });
  Object.defineProperty(win.frames, '0', { get: () => frames[0] });
  return win;
}

function fakeChild(): Record<string, unknown> {
  return {
    RTCPeerConnection: function RTCPeerConnection() {},
    navigator: { mediaDevices: { getUserMedia: () => 'camera' } },
    frames: { length: 0 },
    document: {},
  };
}

test('the page guard takes WebRTC and media capture away from the page and the frames it makes', () => {
  const win = fakeWindow();
  runInNewContext(guardScript(), { window: win, Object });
  assert.equal(win.RTCPeerConnection, undefined);
  assert.equal(win.RTCDataChannel, undefined);
  const navigator = win.navigator as Record<string, unknown>;
  assert.equal(navigator.mediaDevices, undefined);
  assert.equal(navigator.getUserMedia, undefined);
  const MediaDevicesClass = win.MediaDevices as { prototype: Record<string, unknown> };
  assert.equal(MediaDevicesClass.prototype.getUserMedia, undefined);
  assert.equal(MediaDevicesClass.prototype.getDisplayMedia, undefined);
  // It cannot be put back.
  assert.throws(() => {
    'use strict';
    Object.defineProperty(win, 'RTCPeerConnection', { value: () => 'peer' });
  });
  // A frame read through contentWindow is guarded before the page gets it.
  const Frame = win.HTMLIFrameElement as new () => { contentWindow: Record<string, unknown>; contentDocument: { defaultView: Record<string, unknown> } };
  const first = new Frame();
  assert.equal(first.contentWindow.RTCPeerConnection, undefined);
  assert.equal((first.contentWindow.navigator as Record<string, unknown>).mediaDevices, undefined);
  const second = new Frame();
  assert.equal(second.contentDocument.defaultView.RTCPeerConnection, undefined);
  // A frame inserted and then reached through window.frames is guarded by the insertion.
  const third = new Frame();
  const NodeClass = win.Node as { prototype: { appendChild(node: unknown): unknown } };
  NodeClass.prototype.appendChild.call({}, third);
  const reached = (win.frames as Record<string, Record<string, unknown>>)['0'];
  assert.equal(reached.RTCPeerConnection, undefined);
  // Running twice (the page's own copy after the window's) changes nothing.
  runInNewContext(guardScript(), { window: win, Object });
  assert.equal(win.RTCPeerConnection, undefined);
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


test('the preview says exactly what the internet switch does, on each system', () => {
  for (const text of [PREVIEW_NETWORK_OFF_TEXT_WINDOWS, PREVIEW_NETWORK_OFF_TEXT_MAC, PREVIEW_NETWORK_ON_TEXT]) {
    assert.match(text, /window of its own/);
    // No promise that WebRTC is off: on a Mac it is not, and nowhere is a page script the block.
    assert.doesNotMatch(text, /WebRTC (and DNS look-ahead )?(is|are) off/);
    assert.doesNotMatch(text, /sealed|no network/i);
  }
  // Windows: the window's own browser has a dead proxy, so WebRTC cannot leave either.
  assert.match(PREVIEW_NETWORK_OFF_TEXT_WINDOWS, /closed address/);
  assert.match(PREVIEW_NETWORK_OFF_TEXT_WINDOWS, /WebRTC included/);
  // Mac: said plainly.
  assert.match(PREVIEW_NETWORK_OFF_TEXT_MAC, /might still reach the internet through WebRTC/);
  assert.match(PREVIEW_NETWORK_ON_TEXT, /https/);
  assert.equal(previewNetworkText(false, false), PREVIEW_NETWORK_OFF_TEXT_WINDOWS);
  assert.equal(previewNetworkText(false, true), PREVIEW_NETWORK_OFF_TEXT_MAC);
  assert.equal(previewNetworkText(true, true), PREVIEW_NETWORK_ON_TEXT);
  const page = read('../src/features/artifacts/ArtifactPage.tsx');
  assert.match(page, /previewNetworkText\(network, IS_MAC\)/);
  // The Windows claim rests on the window's arguments; the rest on the page guard (preview.rs).
  const window = read('../src-tauri/src/creations/preview_window.rs');
  assert.match(window, /pub const DEAD_PROXY: &str = "--proxy-server=127\.0\.0\.1:9";/);
  assert.match(window, /\.initialization_script_for_all_frames\(preview::GUARD_SCRIPT\)/);
  const rust = read('../src-tauri/src/creations/preview.rs');
  assert.match(rust, /\.header\("X-DNS-Prefetch-Control", "off"\)/);
  assert.match(rust, /'RTCPeerConnection'/);
  // No code comment calls WebRTC off either.
  for (const file of ['../src-tauri/src/creations/preview.rs', '../src-tauri/src/creations/preview_window.rs', '../src/features/artifacts/model.ts']) {
    assert.doesNotMatch(read(file), /WebRTC (and DNS look-ahead )?(is|are) off/, file);
  }
});

test('the pane says the page is live once it loaded, even when the load came first or long ago', () => {
  // A page kept from before (the pane opened again on the same preview): Rust says it loaded.
  assert.equal(previewLoadAtEmbed(true, []), 'loaded');
  // A fresh page that finished before the embed answered: its early report counts.
  assert.equal(previewLoadAtEmbed(false, ['loaded']), 'loaded');
  assert.equal(previewLoadAtEmbed(false, []), 'opening');
  // Blocked stays blocked (it is shown with the page), a stop wins.
  assert.equal(previewLoadAtEmbed(true, ['blocked']), 'blocked');
  assert.equal(previewLoadAtEmbed(false, ['blocked', 'loaded']), 'blocked');
  assert.equal(previewLoadAtEmbed(true, ['stopped']), 'stopped');
  assert.equal(previewLoadAfter('opening', 'loaded'), 'loaded');
  assert.equal(previewLoadAfter('blocked', 'loaded'), 'blocked');
  assert.equal(previewLoadAfter('loaded', 'blocked'), 'blocked');
  assert.equal(previewLoadAfter('loaded', 'stopped'), 'stopped');
  // A reload that loads again: live again.
  assert.equal(previewLoadAfter('opening', 'loaded'), 'loaded');
});

test('the status line says why the page is not on screen, never Opening over a shown page', () => {
  const base = { error: null, end: null, load: 'loaded' as const, hiddenBy: null, version: null, poppedOut: false };
  assert.equal(previewStateLine(base), 'Now · live');
  assert.equal(previewStateLine({ ...base, version: 2 }), 'Version 2 · live');
  assert.equal(previewStateLine({ ...base, hiddenBy: 'Design check' }), 'Hidden while Design check is open');
  // Hidden says so even before the first load is reported.
  assert.equal(previewStateLine({ ...base, load: 'opening', hiddenBy: 'a menu' }), 'Hidden while a menu is open');
  assert.equal(previewStateLine({ ...base, load: 'opening' }), 'Opening');
  assert.equal(previewStateLine({ ...base, load: 'stopped' }), 'Stopped');
  assert.equal(previewStateLine({ ...base, end: 'closed' }), PREVIEW_END_TEXT.closed);
  assert.equal(previewStateLine({ ...base, error: 'boom' }), 'Could not open');
  // Popped out, nothing in this window hides it.
  assert.equal(previewStateLine({ ...base, poppedOut: true, hiddenBy: 'a menu' }), 'Now · in its own window');
});

test('the page opens itself once the folder has its entry page', () => {
  assert.equal(hasEntryPage(null), false);
  assert.equal(hasEntryPage([]), false);
  assert.equal(hasEntryPage([{ path: 'style.css', size: 1 }, { path: 'src/index.html', size: 1 }]), false);
  assert.equal(hasEntryPage([{ path: 'style.css', size: 1 }, { path: 'index.html', size: 12 }]), true);
  // The session checks the folder after each finished building step and offers the page once.
  const session = read('../src/features/ask/ChatSessionContext.tsx');
  assert.match(session, /hasEntryPage\(files\)/);
  assert.match(session, /event\.type === 'step_finished'/);
  assert.match(session, /whenWatching: true/);
  // Rust says whether the page loaded already, so a pane opened again is not stuck on Opening.
  const rust = read('../src-tauri/src/creation_commands.rs');
  assert.match(rust, /loaded: preview_window::has_loaded\(&/);
});

test('the page is the main thing on screen: one header row, the page fills the rest', () => {
  const page = read('../src/features/artifacts/ArtifactPage.tsx');
  const embedded = read('../src/features/artifacts/EmbeddedPreview.tsx');
  // The header never wraps; its title gives way first.
  assert.match(page, /<header className="[^"]*flex-nowrap[^"]*"/);
  assert.doesNotMatch(page, /<header className="[^"]*flex-wrap/);
  // What was built and chosen is folded away under the header, not stacked above the page.
  assert.match(page, /<details\b/);
  // The preview's box takes the height that is left, not a fixed share of the window.
  assert.match(embedded, /ref=\{boxRef\}[\s\S]*?className="[^"]*flex-1 min-h-0/);
  assert.doesNotMatch(embedded, /h-\[min\(72vh/);
  // What hides the page is said, by name.
  assert.match(embedded, /previewStateLine\(\{/);
  assert.match(embedded, /coveringOverlay\(/);
});
