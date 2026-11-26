/**
 * The page frame (components/shell/PageFrame.tsx): one left edge, one width and one title
 * position for every view but Ask, a reserved slot for the primary action, and a corner
 * kept clear for the floating chat's pill. And Ask's message box, which sits at the bottom
 * before and after the first message. Read from the sources (the tests have no DOM).
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/pageFrame.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  DOCK_GUTTER_PX,
  PAGE_ACTION_SLOT_CLASS,
  PAGE_BOTTOM_PX,
  PAGE_COLUMN_CLASS,
  PAGE_END_CLASS,
  PAGE_MAX_WIDTH_PX,
  PAGE_SIDE_PX,
  PAGE_TITLE_CLASS,
  PAGE_TITLE_ROW_CLASS,
  PAGE_TITLE_ROW_PX,
  PAGE_TOP_PX,
  PAGE_WIDTH_CLASS,
} from '../src/components/shell/frameMeasures.ts';
import { EXTRA_SPACING } from '../src/lib/spacingScale.ts';
import { TYPE_SCALE } from '../src/lib/typeScale.ts';

const source = (path: string) => readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8');
const withoutComments = (text: string) => text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');

/** Every view but Ask, by its file. */
const VIEWS: Record<string, string> = {
  artifacts: 'features/artifacts/ArtifactsView.tsx',
  workspaces: 'features/workspaces/WorkspacesView.tsx',
  library: 'features/library/LibraryView.tsx',
  connectors: 'features/connectors/ConnectorsView.tsx',
  tasks: 'features/tasks/TasksView.tsx',
  automations: 'features/automations/AutomationsView.tsx',
  activity: 'features/activity/ActivityView.tsx',
  settings: 'components/shell/SettingsView.tsx',
};

/** Tailwind's spacing step, in CSS pixels. */
const step = (n: number) => n * 4;

test('the frame is every view but Ask', () => {
  // The views the shell loads (`views.ts`) are the ones framed here: a new view is added to both.
  const loaders = [...source('components/shell/views.ts').matchAll(/^ {2}(\w+): \(\) => import\('\.\.?\/[^']+'\),$/gm)].map(m => m[1]);
  assert.deepEqual([...loaders].sort(), Object.keys(VIEWS).sort());
  for (const [view, file] of Object.entries(VIEWS)) {
    const text = withoutComments(source(file));
    assert.match(text, /<PageFrame\b/, `${view} is drawn in the page frame`);
    // The frame writes the page's one h1: a view writes none of its own.
    assert.doesNotMatch(text, /<h1\b/, `${view} writes its own h1`);
    // And none sets its own page width or side padding beside the frame's.
    assert.doesNotMatch(text, /max-w-5xl mx-auto px-/, `${view} builds its own frame`);
  }
});

test("each view's title is the name its tab has", () => {
  const titles: Record<string, string> = {
    artifacts: 'Artifacts', workspaces: 'Workspaces', library: 'Library', connectors: 'Connectors',
    tasks: 'Tasks', automations: 'Automations', activity: 'Activity', settings: 'Settings',
  };
  for (const [view, file] of Object.entries(VIEWS)) {
    assert.match(source(file), new RegExp(`title="${titles[view]}"`), view);
  }
});

test("the frame's classes say its measures", () => {
  // One left edge and one top: px-8 and pt-7.
  assert.match(PAGE_COLUMN_CLASS, /(?<![\w-])px-8(?![\w-])/);
  assert.equal(step(8), PAGE_SIDE_PX);
  assert.match(PAGE_COLUMN_CLASS, /(?<![\w-])pt-7(?![\w-])/);
  assert.equal(step(7), PAGE_TOP_PX);
  // One width: max-w-5xl is 64rem.
  assert.match(PAGE_WIDTH_CLASS, /(?<![\w-])max-w-5xl(?![\w-])/);
  assert.equal(64 * 16, PAGE_MAX_WIDTH_PX);
  // The title row: 36px, the title's one size, the action's slot pushed to its end.
  assert.match(PAGE_TITLE_ROW_CLASS, /(?<![\w-])min-h-9(?![\w-])/);
  assert.equal(step(9), PAGE_TITLE_ROW_PX);
  assert.match(PAGE_TITLE_CLASS, /(?<![\w-])text-title(?![\w-])/);
  assert.equal(TYPE_SCALE.title, '20px');
  assert.match(PAGE_ACTION_SLOT_CLASS, /(?<![\w-])ms-auto(?![\w-])/);
  assert.match(PAGE_ACTION_SLOT_CLASS, /(?<![\w-])min-h-9(?![\w-])/);
  // The page's end: 28px and the pill's gutter.
  assert.match(PAGE_COLUMN_CLASS, new RegExp(`(?<![\\w-])${PAGE_END_CLASS}(?![\\w-])`));
  assert.equal(PAGE_END_CLASS, 'pb-page-end');
  assert.equal(EXTRA_SPACING['page-end'], `${PAGE_BOTTOM_PX}px`);
  assert.equal(PAGE_BOTTOM_PX, 28 + DOCK_GUTTER_PX);
});

test("the gutter is the pill's own height and distance from the window's edge", () => {
  const dock = source('components/shell/DockPanel.tsx');
  // The pill: fixed, 16px above the bottom edge (bottom-4), 40px tall (h-10).
  assert.match(dock, /const CORNER = 'fixed bottom-4 right-4 z-40';/);
  assert.match(dock, /'dock-pop inline-flex items-center gap-2 h-10 /);
  assert.equal(step(4) + step(10), DOCK_GUTTER_PX);
});

test('the frame draws the title, the reserved action slot, and the gutter itself', () => {
  const frame = withoutComments(source('components/shell/PageFrame.tsx'));
  assert.match(frame, /<h1 id=\{titleId\} className=\{PAGE_TITLE_CLASS\} data-page-title="">/);
  // The slot is there whether or not the view has an action: nothing shifts when one appears.
  assert.match(frame, /<div className=\{PAGE_ACTION_SLOT_CLASS\} data-page-action="">\s*\{action\}\s*<\/div>/);
  assert.match(frame, /className=\{cn\(PAGE_COLUMN_CLASS, width === 'page' && PAGE_WIDTH_CLASS\)\}/);
  // A frame whose panes scroll by themselves has the same left edge and top.
  assert.match(frame, /'shrink-0 w-full px-8 pt-7 pb-4 flex flex-col gap-4'/);
});

test('a view whose panes scroll by themselves ends each pane with the gutter', () => {
  // Tasks (list and calendar), Automations (its detail pane) and Settings (its section).
  for (const file of ['components/CalendarTodoPanel.tsx', 'features/tasks/TasksCalendar.tsx', 'features/automations/AutomationsView.tsx', 'components/shell/SettingsView.tsx']) {
    assert.match(source(file), /(?<![\w-])pb-page-end(?![\w-])/, file);
  }
  for (const view of ['tasks', 'automations', 'settings']) {
    assert.match(source(VIEWS[view]), /<PageFrame\s+fill\b/, view);
  }
});

test('a page opened inside a view keeps the frame and writes its own heading', () => {
  assert.match(source(VIEWS.workspaces), /heading=\{!selected\}/);
  assert.match(source(VIEWS.connectors), /heading=\{!page\}/);
  assert.match(source(VIEWS.library), /<PageFrame title="Library" heading=\{false\}>/);
  // The paper graph is a page of its own, with the way back above its title.
  assert.match(source(VIEWS.library), /<PageFrame title="Paper graph" lede="How your papers cite each other" back=\{\{ label: 'Library', onClick: popPage \}\}>/);
});

test('loading, empty and failed are drawn one way', () => {
  const state = withoutComments(source('components/shell/ViewState.tsx'));
  assert.match(state, /export type ViewStateKind = 'empty' \| 'loading' \| 'error' \| 'offline';/);
  // An error is announced at once; the others when the reader gets there.
  assert.match(state, /role=\{kind === 'error' \? 'alert' : 'status'\}/);
  for (const view of ['artifacts', 'workspaces', 'automations']) {
    const text = source(VIEWS[view]);
    assert.match(text, /<ViewState kind="loading"/, `${view}: loading`);
    assert.match(text, /<ViewState kind="error"/, `${view}: error`);
  }
  assert.match(source(VIEWS.artifacts), /<ViewState kind="empty"/);
});

test("Ask's message box is one element at the bottom, before and after the first message", () => {
  const ask = withoutComments(source('features/ask/AskView.tsx'));
  // One composer, in one dock, measured once: there is no second layout for a new chat.
  assert.equal(ask.match(/\{composer\(true\)\}/g)?.length, 1);
  assert.equal(ask.match(/ref=\{setComposerArea\}/g)?.length, 1);
  assert.match(ask, /<div ref=\{setComposerArea\} className="absolute left-0 right-0 bottom-0 /);
  // A new chat changes what is above the box, not the box.
  assert.match(ask, /\{empty \? emptyChat : \(/);
  assert.doesNotMatch(ask, /if \(messages\.length === 0\) \{\s*return \(/);
  assert.equal(ask.match(/<section aria-label="Ask"/g)?.length, 1);
});
