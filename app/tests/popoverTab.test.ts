/**
 * Tab in a pop-up (InfoPopover): its controls are reached in order, a link in its
 * text included, and past the last (or back from the first) it closes. Run with
 * Node 22.6+:
 *   node --experimental-strip-types --test app/tests/popoverTab.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { popoverTab } from '../src/lib/popoverTab.ts';

test('Tab moves through the close button and then a link in the text', () => {
  // Close button (0), a link in the explanation (1).
  assert.equal(popoverTab(-1, 2, false), 'move', 'from the pop-up to Close');
  assert.equal(popoverTab(0, 2, false), 'move', 'from Close to the link: it is reachable');
  assert.equal(popoverTab(1, 2, false), 'close', 'past the last control');
});

test('Shift+Tab goes back, and back from the first control or the pop-up closes', () => {
  assert.equal(popoverTab(1, 2, true), 'move');
  assert.equal(popoverTab(0, 2, true), 'close');
  assert.equal(popoverTab(-1, 2, true), 'close');
});

test('with only its close button, Tab past it closes', () => {
  assert.equal(popoverTab(-1, 1, false), 'move');
  assert.equal(popoverTab(0, 1, false), 'close');
});

test('the pop-up is described by its text and uses this Tab cycle', () => {
  const source = readFileSync(new URL('../src/components/ui/InfoPopover.tsx', import.meta.url), 'utf8');
  assert.match(source, /aria-describedby=\{bodyId\}/);
  assert.match(source, /id=\{bodyId\}/);
  assert.match(source, /popoverTab\(/);
});
