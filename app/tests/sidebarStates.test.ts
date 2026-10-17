/**
 * Hover and current-item states on the sidebar must be visible against it.
 * In the light theme `raised` is the sidebar's own colour, so a state drawn
 * with `bg-shodh-raised` (the open chat, a hovered row) cannot be seen.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/sidebarStates.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8');

/** `--c-<name>` in each theme block of index.css, in order (dark, then light). */
function tokens(css: string, name: string): string[] {
  return [...css.matchAll(new RegExp(`--c-${name}:\\s*([^;]+);`, 'g'))].map(m => m[1].trim().toLowerCase());
}

/** Drawn on the sidebar throughout (their menus and popovers sit on `surface`). */
const SIDEBAR_FILES = [
  '../src/components/shell/Sidebar.tsx',
  '../src/components/shell/ConversationHistory.tsx',
];

/** A hover, open or current-item state drawn with `raised` rather than `raised-2`. */
const INVISIBLE_STATE = /(?:hover:bg-shodh-raised|(?:open|isActive|active)\s*(?:&&|\?)\s*'bg-shodh-raised)(?![-\w])/g;

test('the light sidebar is the same colour as raised, and raised-2 differs from it', () => {
  const css = read('../src/index.css');
  const sidebar = tokens(css, 'sidebar');
  const raised = tokens(css, 'raised');
  const raised2 = tokens(css, 'raised-2');
  assert.equal(sidebar.length, 2);
  // The premise of this test: if the themes change so raised differs from the
  // sidebar, the rule below can be relaxed deliberately, not by accident.
  assert.equal(raised[1], sidebar[1]);
  for (const i of [0, 1]) assert.notEqual(raised2[i], sidebar[i]);
});

for (const file of SIDEBAR_FILES) {
  test(`${file.split('/').pop()} draws hover and current states with raised-2`, () => {
    const found = read(file).match(INVISIBLE_STATE) ?? [];
    assert.deepEqual(found, []);
  });
}

test('the Activity button, on the sidebar, shows hover and open with raised-2', () => {
  // Its panel is on `surface`, where raised is visible, so only the button is checked.
  const source = read('../src/components/shell/ActivityTray.tsx');
  assert.match(source, /hover:bg-shodh-raised-2\/60 hover:text-shodh-text transition-colors duration-micro',\s*open && 'bg-shodh-raised-2 text-shodh-text'/);
});

test('the open chat in the history is marked with raised-2', () => {
  const source = read('../src/components/shell/ConversationHistory.tsx');
  assert.match(source, /isActive \? 'bg-shodh-raised-2 text-shodh-text'/);
});
