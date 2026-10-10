/**
 * View id tests. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/viewTabs.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  VIEW_TABS,
  VIEW_TAB_DESCRIPTIONS,
  VIEW_TAB_KEYWORDS,
  VIEW_TAB_LABELS,
  isViewTab,
  normalizeViewTab,
} from '../src/lib/viewTabs.ts';

test('the shell has exactly the shipped views, in sidebar order', () => {
  assert.deepEqual([...VIEW_TABS], ['ask', 'workspaces', 'library', 'tasks', 'activity', 'settings']);
});

test('every view has a label, a description and keywords', () => {
  for (const view of VIEW_TABS) {
    assert.ok(VIEW_TAB_LABELS[view], `label for ${view}`);
    assert.ok(VIEW_TAB_DESCRIPTIONS[view], `description for ${view}`);
    assert.ok(VIEW_TAB_KEYWORDS[view], `keywords for ${view}`);
  }
});

test('current ids resolve to themselves', () => {
  for (const view of VIEW_TABS) {
    assert.equal(isViewTab(view), true);
    assert.equal(normalizeViewTab(view), view);
  }
});

test('legacy and agent ids resolve through aliases', () => {
  assert.equal(normalizeViewTab('chat'), 'ask');
  assert.equal(normalizeViewTab('documents'), 'library');
  // The open_view tool enum (Rust) still emits 'calendar'.
  assert.equal(normalizeViewTab('calendar'), 'tasks');
  assert.equal(normalizeViewTab('audit'), 'activity');
  assert.equal(isViewTab('calendar'), false);
});

test('unknown, hidden and non-string ids are not navigable', () => {
  for (const value of ['graph', 'terminal', 'automations', '', 'Ask', 'toString', '__proto__', null, undefined, 3, {}]) {
    assert.equal(normalizeViewTab(value), null, `value ${String(value)}`);
  }
});
