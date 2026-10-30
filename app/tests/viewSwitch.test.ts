/**
 * Switching views never shows an empty pane: <main> and its Suspense boundary
 * stay mounted, the shown view is a deferred copy of the requested one (so the
 * current view stays until the next has rendered), and the next view enters
 * from most of its opacity. One-shot requests a view applies on mount are read
 * during render and taken after it, because a deferred render can run again
 * before it shows. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/viewSwitch.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { VIEW_TABS } from '../src/lib/viewTabs.ts';
import { peekArchivedRequest, requestArchivedWorkspaces, takeArchivedRequest } from '../src/features/workspaces/archivedRequest.ts';
import { peekRequestedConnectorPage, requestConnectorPage, takeRequestedConnectorPage } from '../src/features/connectors/navigation.ts';

const app = readFileSync(new URL('../src/App-SplitView.tsx', import.meta.url), 'utf8');
const css = readFileSync(new URL('../src/index.css', import.meta.url), 'utf8');

function mainElement(): string {
  const start = app.search(/<main\s+id="main-content"/);
  assert.ok(start > 0, '<main> is rendered');
  return app.slice(start, app.indexOf('</main>', start));
}

test('<main> is not remounted on a switch, and its Suspense boundary sits above the keyed frame', () => {
  const main = mainElement();
  const openingTag = main.slice(0, main.indexOf('>') + 1);
  assert.doesNotMatch(openingTag, /\bkey=/, '<main> has no key');
  assert.doesNotMatch(openingTag, /shell-view-enter/, '<main> itself does not animate');
  assert.match(openingTag, /aria-label=\{VIEW_TAB_LABELS\[viewTab\]\}/);
  assert.match(openingTag, /id="main-content"/);
  assert.match(openingTag, /tabIndex=\{-1\}/);
  const suspense = main.indexOf('<Suspense fallback={<ViewSkeleton view={viewTab} />}>');
  const frame = main.indexOf('<div key={viewTab} data-view={viewTab} className="shell-view-enter h-full">');
  assert.ok(suspense > 0, 'one Suspense boundary inside <main>');
  assert.ok(frame > suspense, 'the keyed frame is inside the boundary, so the boundary is never remounted');
  assert.equal(main.split('<Suspense').length - 1, 1);
});

test('the shown view follows the requested one once it can render, and every view renders inside the frame', () => {
  // Ready views switch in the same commit; a view whose code is still loading
  // is shown through the deferred value, so the current one stays until then.
  assert.match(app, /const deferredTab = useDeferredValue\(activeTab\);/);
  assert.match(app, /const viewTab = isViewReady\(activeTab\) \? activeTab : deferredTab;/);
  const main = mainElement();
  assert.doesNotMatch(main, /activeTab ===/, 'the content follows the shown view only');
  for (const tab of VIEW_TABS) {
    assert.match(main, new RegExp(`\{viewTab === '${tab}' && `), `${tab} renders when shown`);
  }
  // The sidebar follows the click at once; the title and the dock follow the shown view.
  assert.match(app, /activeView=\{activeTab\}/);
  assert.match(app, /\{VIEW_TAB_LABELS\[viewTab\]\}/);
  assert.match(app, /<ConversationDock activeTab=\{viewTab\}/);
});

test('a view enters from most of its opacity, never from blank, and not at all under reduced motion', () => {
  assert.match(css, /\.shell-view-enter\s*\{\s*animation:\s*shell-view-in var\(--dur-panel\) var\(--ease-enter\) both;/);
  const keyframes = /@keyframes shell-view-in\s*\{\s*from\s*\{\s*opacity:\s*([\d.]+);/.exec(css);
  assert.ok(keyframes, 'shell-view-in starts from an explicit opacity');
  assert.ok(Number(keyframes[1]) >= 0.5, `starts at opacity ${keyframes[1]}`);
  assert.match(css, /@media \(prefers-reduced-motion: reduce\)\s*\{\s*\.shell-view-enter,/);
});

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap(name => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return sourceFiles(path);
    return /\.(ts|tsx)$/.test(name) ? [path] : [];
  });
}

test('no first render takes a one-shot request (it may render again before it shows)', () => {
  const src = fileURLToPath(new URL('../src', import.meta.url));
  const offenders: string[] = [];
  for (const file of sourceFiles(src)) {
    const text = readFileSync(file, 'utf8');
    for (const match of text.matchAll(/use(?:State|Memo|Ref)\b[^;]*?\btake[A-Z]\w*\(/g)) {
      offenders.push(`${file}: ${match[0].slice(0, 80)}`);
    }
  }
  assert.deepEqual(offenders, []);
});

test('reading a request leaves it for the view to take once', () => {
  const target = { dispatchEvent: () => true };
  takeArchivedRequest();
  requestArchivedWorkspaces(target);
  assert.equal(peekArchivedRequest(), true);
  assert.equal(peekArchivedRequest(), true, 'a second render reads it too');
  assert.equal(takeArchivedRequest(), true);
  assert.equal(peekArchivedRequest(), false);

  takeRequestedConnectorPage();
  requestConnectorPage({ connector: 'telegram', tab: null });
  assert.deepEqual(peekRequestedConnectorPage(), { connector: 'telegram', tab: null });
  assert.deepEqual(peekRequestedConnectorPage(), { connector: 'telegram', tab: null });
  assert.deepEqual(takeRequestedConnectorPage(), { connector: 'telegram', tab: null });
  assert.equal(peekRequestedConnectorPage(), null);
});

test('views read their mount-time requests and take them in an effect', () => {
  const read = (path: string) => readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8');
  assert.match(read('components/shell/SettingsView.tsx'), /useState<SettingsSection>\(\(\) => peekRequestedSection\(\) \?\? 'models'\)/);
  assert.match(read('features/connectors/ConnectorsView.tsx'), /useState<ConnectorPage \| null>\(\(\) => peekRequestedConnectorPage\(\) \?\? lastConnectorPage\(\)\)/);
  assert.match(read('features/workspaces/WorkspacesView.tsx'), /useState\(\(\) => peekArchivedRequest\(\)\)/);
  assert.match(read('features/modelPicker/ConnectPanel.tsx'), /useState<ProviderId \| null>\(\(\) => peekFixKeyRequest\(\)\)/);
  assert.match(read('components/MemorySettings.tsx'), /peekRequestedMemoryEdit\(\)\)\);/);
  assert.match(read('components/MemorySettings.tsx'), /if \(!props\.scope\) takeRequestedMemoryEdit\(\);/);
});
