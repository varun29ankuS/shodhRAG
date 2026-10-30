/**
 * Switching views never shows an empty pane: the view shown follows the one
 * asked for in the same commit when its code is loaded, else the current one
 * stays until it can render; <main> and its Suspense boundary stay mounted;
 * the next view enters from most of its opacity. One-shot requests a view
 * applies on mount are read during render and taken after it, because a
 * deferred render can run again before it shows. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/viewSwitch.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { VIEW_TABS } from '../src/lib/viewTabs.ts';
import { shownView } from '../src/lib/viewSwitch.ts';
import { peekArchivedRequest, requestArchivedWorkspaces, takeArchivedRequest } from '../src/features/workspaces/archivedRequest.ts';
import { peekRequestedConnectorPage, requestConnectorPage, takeRequestedConnectorPage } from '../src/features/connectors/navigation.ts';

test('a view whose code is loaded is shown at once; one still loading leaves the current view up', () => {
  const loaded = new Set(['ask', 'library']);
  const ready = (view: string) => loaded.has(view);
  assert.equal(shownView('library', 'ask', ready), 'library', 'same commit as the click');
  assert.equal(shownView('tasks', 'ask', ready), 'ask', 'Ask stays until Tasks can render');
  assert.equal(shownView('tasks', 'tasks', ready), 'tasks', 'shown once the deferred render commits');
  loaded.add('tasks');
  assert.equal(shownView('tasks', 'ask', ready), 'tasks', 'shown as soon as its code is in, without waiting on the deferred render');
});

const app = readFileSync(new URL('../src/App-SplitView.tsx', import.meta.url), 'utf8');
const css = readFileSync(new URL('../src/index.css', import.meta.url), 'utf8');

/** The JSX of <main id="main-content"> … </main>. */
function mainElement(): string {
  const start = app.search(/<main\s+id="main-content"/);
  assert.ok(start > 0, '<main id="main-content"> is rendered');
  return app.slice(start, app.indexOf('</main>', start));
}

test('<main> is never remounted, and the only remounted frame sits inside the Suspense boundary', () => {
  const main = mainElement();
  const openingTag = main.slice(0, main.indexOf('>') + 1);
  assert.doesNotMatch(openingTag, /\bkey=/, '<main> has no key');
  assert.match(openingTag, /aria-label=\{VIEW_TAB_LABELS\[viewTab\]\}/, 'main is named after the view shown');
  const suspense = main.indexOf('<Suspense');
  const frame = main.search(/key=\{viewTab\}/);
  assert.ok(suspense > 0 && frame > suspense, 'the keyed frame is inside the boundary');
  assert.equal(main.split('<Suspense').length - 1, 1, 'one boundary');
  assert.match(app, /useDeferredValue\(activeTab\)/);
  assert.match(app, /shownView\(activeTab, /);
});

test('every view renders inside the frame, from the view shown', () => {
  const main = mainElement();
  assert.doesNotMatch(main, /activeTab ===/);
  for (const tab of VIEW_TABS) assert.match(main, new RegExp(String.raw`viewTab === '${tab}'`), `${tab} renders when shown`);
});

test('a view enters from most of its opacity, never from blank, and not at all under reduced motion', () => {
  const rule = /\.shell-view-enter\s*\{\s*animation:\s*([\w-]+)\s/.exec(css);
  assert.ok(rule, '.shell-view-enter animates');
  const keyframes = new RegExp(String.raw`@keyframes ${rule[1]}\s*\{\s*from\s*\{\s*opacity:\s*([\d.]+);`).exec(css);
  assert.ok(keyframes, `${rule[1]} starts from an explicit opacity`);
  assert.ok(Number(keyframes[1]) >= 0.5, `starts at opacity ${keyframes[1]}`);
  const reduced = [...css.matchAll(/@media \(prefers-reduced-motion: reduce\)\s*\{([^}]*)\{\s*animation: none;/g)].map(m => m[1]);
  assert.ok(reduced.some(list => /\.shell-view-enter\b/.test(list)));
});

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap(name => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return sourceFiles(path);
    return /\.(ts|tsx)$/.test(name) ? [path] : [];
  });
}

/** The argument list of the call whose `(` is at `open`, by balanced parentheses (strings and comments are rare enough here to ignore). */
function argumentsAt(text: string, open: number): string {
  let depth = 0;
  for (let i = open; i < text.length; i += 1) {
    if (text[i] === '(') depth += 1;
    else if (text[i] === ')') {
      depth -= 1;
      if (depth === 0) return text.slice(open + 1, i);
    }
  }
  return text.slice(open + 1);
}

/** Hooks whose arguments run during render. */
const RENDER_HOOKS = /\buse(State|Reducer|Memo|Ref|DeferredValue|SyncExternalStore)\s*(<[^()]*?>)?\s*\(/g;

test('no render takes a one-shot request (it may render again before it shows)', () => {
  const src = fileURLToPath(new URL('../src', import.meta.url));
  const offenders: string[] = [];
  for (const file of sourceFiles(src)) {
    const text = readFileSync(file, 'utf8');
    for (const match of text.matchAll(RENDER_HOOKS)) {
      const args = argumentsAt(text, (match.index ?? 0) + match[0].length - 1);
      if (/\btake[A-Z]\w*\s*\(/.test(args)) offenders.push(`${file}: ${match[0]}${args.slice(0, 60)}`);
    }
  }
  assert.deepEqual(offenders, []);
});

test('the scan catches a take inside any render hook, and nothing outside one', () => {
  const find = (text: string) =>
    [...text.matchAll(RENDER_HOOKS)].filter(m => /\btake[A-Z]\w*\s*\(/.test(argumentsAt(text, (m.index ?? 0) + m[0].length - 1))).length;
  assert.equal(find('const [a] = useState(() => takeRequest());'), 1);
  assert.equal(find('const [s, d] = useReducer(reducer, null, () => takeRequest());'), 1);
  assert.equal(find('const x = useMemo<Page | null>(() => takePage(), []);'), 1);
  assert.equal(find('const [a] = useState(0); useEffect(() => { takeRequest(); }, []);'), 0, 'statements after the hook are not its arguments');
  assert.equal(find('const [a] = useState(() => peekRequest());'), 0);
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
