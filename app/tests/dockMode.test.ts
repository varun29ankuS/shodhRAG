/**
 * Conversation dock layout tests: it floats in the corner, never a column. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/dockMode.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { RETIRED_DOCK_STORAGE_KEYS, dockLayout } from '../src/lib/dockMode.ts';

const dock = readFileSync(new URL('../src/components/shell/ConversationDock.tsx', import.meta.url), 'utf8');
// The pill and the panel's markup, shared with the floating chat over the embedded preview.
const panel = readFileSync(new URL('../src/components/shell/DockPanel.tsx', import.meta.url), 'utf8');

test('closed it is a pill; open it floats', () => {
  assert.equal(dockLayout(false), 'pill');
  assert.equal(dockLayout(true), 'overlay');
});

test('the dock never renders a side panel the page reflows around', () => {
  for (const source of [dock, panel]) {
    assert.doesNotMatch(source, /layout === 'panel'/);
    assert.doesNotMatch(source, /role="separator"/);
    assert.doesNotMatch(source, /Pin as side panel/);
  }
  // Pill and chat both sit fixed in the bottom-right corner, over the page.
  assert.match(panel, /const CORNER = 'fixed bottom-4 right-4 z-40';/);
  const corner = panel.match(/fixed \? cn\(CORNER, /g) ?? [];
  assert.equal(corner.length, 2);
  // The in-page chat is the fixed one.
  assert.match(dock, /<DockPanel[\s\S]*?\n\s+fixed\n/);
});

test('pressing the page does not collapse the floating chat', () => {
  assert.doesNotMatch(dock, /addEventListener\('pointerdown'/);
  assert.doesNotMatch(panel, /addEventListener\('pointerdown'/);
});

test('the side-panel records of earlier versions are removed, not read', () => {
  assert.deepEqual([...RETIRED_DOCK_STORAGE_KEYS], ['shodh.conversationDock', 'shodh.conversationDock.v2']);
  assert.match(dock, /RETIRED_DOCK_STORAGE_KEYS\.forEach\(key => window\.localStorage\.removeItem\(key\)\)/);
  assert.doesNotMatch(dock, /localStorage\.(getItem|setItem)/);
});
