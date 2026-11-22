/**
 * Conversation dock layout tests: it floats in the corner, never a column. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/dockMode.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { RETIRED_DOCK_STORAGE_KEYS, dockLayout } from '../src/lib/dockMode.ts';

const dock = readFileSync(new URL('../src/components/shell/ConversationDock.tsx', import.meta.url), 'utf8');

test('closed it is a pill; open it floats', () => {
  assert.equal(dockLayout(false), 'pill');
  assert.equal(dockLayout(true), 'overlay');
});

test('the dock never renders a side panel the page reflows around', () => {
  assert.doesNotMatch(dock, /layout === 'panel'/);
  assert.doesNotMatch(dock, /role="separator"/);
  assert.doesNotMatch(dock, /Pin as side panel/);
  // Pill and chat both sit fixed in the bottom-right corner, over the page.
  const corner = dock.match(/fixed bottom-4 right-4/g) ?? [];
  assert.equal(corner.length, 2);
});

test('pressing the page does not collapse the floating chat', () => {
  assert.doesNotMatch(dock, /addEventListener\('pointerdown'/);
});

test('the side-panel records of earlier versions are removed, not read', () => {
  assert.deepEqual([...RETIRED_DOCK_STORAGE_KEYS], ['shodh.conversationDock', 'shodh.conversationDock.v2']);
  assert.match(dock, /RETIRED_DOCK_STORAGE_KEYS\.forEach\(key => window\.localStorage\.removeItem\(key\)\)/);
  assert.doesNotMatch(dock, /localStorage\.(getItem|setItem)/);
});
