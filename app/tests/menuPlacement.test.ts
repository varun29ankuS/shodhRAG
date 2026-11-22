/**
 * Where the composer's mode menu opens: never off the window. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/menuPlacement.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { MENU_EDGE, MENU_GAP, placeMenu } from '../src/lib/menuPlacement.ts';

const W = 1280;
const pill = (top: number) => ({ top, bottom: top + 30, left: 400 });

test('a composer low in the window: the menu opens above, whole', () => {
  const p = placeMenu(pill(700), 420, 320, W, 800);
  assert.equal(p.top, 700 - MENU_GAP - 420);
  assert.ok(p.top >= MENU_EDGE);
  assert.equal(p.maxHeight, 700 - MENU_GAP - MENU_EDGE);
});

test('a new chat centres the composer: too little room above, so it opens below', () => {
  // The case that was cut off: 420 px of menu, the pill 250 px from the top.
  const p = placeMenu(pill(250), 420, 320, W, 800);
  assert.equal(p.top, 280 + MENU_GAP);
  assert.ok(p.top + 420 <= 800 - MENU_EDGE);
});

test('room on neither side: the larger side, capped so it scrolls inside the window', () => {
  const p = placeMenu(pill(300), 900, 320, W, 700);
  assert.ok(p.top >= MENU_EDGE, `top ${p.top}`);
  assert.ok(p.top + p.maxHeight <= 700 - MENU_EDGE, `bottom ${p.top + p.maxHeight}`);
  // Below has 700 - 330 - 14 = 356, above 300 - 14 = 286: below wins.
  assert.equal(p.top, 330 + MENU_GAP);
  assert.equal(p.maxHeight, 700 - 330 - MENU_GAP - MENU_EDGE);
});

test('capped above: the menu starts at the window edge', () => {
  const p = placeMenu(pill(500), 900, 320, W, 600);
  assert.equal(p.top, MENU_EDGE);
  assert.equal(p.maxHeight, 500 - MENU_GAP - MENU_EDGE);
});

test('near the right edge it shifts left, never off the window', () => {
  const p = placeMenu({ top: 700, bottom: 730, left: 1100 }, 300, 320, W, 800);
  assert.equal(p.left, W - 320 - MENU_EDGE);
  assert.equal(placeMenu({ top: 700, bottom: 730, left: 2 }, 300, 320, W, 800).left, MENU_EDGE);
});

test('the mode menu is portaled and placed by placeMenu, so no container clips it', () => {
  const picker = readFileSync(new URL('../src/features/agent/ModePicker.tsx', import.meta.url), 'utf8');
  assert.match(picker, /createPortal\(/);
  assert.match(picker, /placeMenu\(/);
  assert.doesNotMatch(picker, /bottom-full/);
  // Inside the portal, a press on the menu is not "outside".
  assert.match(picker, /menuPanelRef\.current\?\.contains\(target\)/);
  assert.match(picker, /data-esc-local/);
});
