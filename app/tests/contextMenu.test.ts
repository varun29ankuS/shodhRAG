/**
 * The webview's page menu (with Reload) is off; its selection menu and text fields' menus stay. Run with
 * Node 22.6+:
 *   node --experimental-strip-types --test app/tests/contextMenu.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EDITABLE_SELECTOR, keepsNativeContextMenu, selectionHasText } from '../src/lib/contextMenu.ts';

/** An element whose `closest` answers for the selectors it is told it matches. */
const inside = (matches: boolean) => ({
  closest: (selector: string) => {
    assert.equal(selector, EDITABLE_SELECTOR);
    return matches ? {} : null;
  },
});

test('without a selection, text fields keep the native menu and the page loses it (Reload)', () => {
  assert.equal(keepsNativeContextMenu(inside(true), false), true);
  assert.equal(keepsNativeContextMenu(inside(false), false), false);
  assert.equal(keepsNativeContextMenu(null, false), false);
});

test('with text selected, the selection menu (Copy) is kept anywhere', () => {
  assert.equal(keepsNativeContextMenu(inside(false), true), true);
  assert.equal(keepsNativeContextMenu(null, true), true);
});

test('a caret or a whitespace-only selection is no selection', () => {
  const sel = (isCollapsed: boolean, text: string) => ({ isCollapsed, toString: () => text });
  assert.equal(selectionHasText(sel(false, 'Revenue grew 12%')), true);
  assert.equal(selectionHasText(sel(true, '')), false);
  assert.equal(selectionHasText(sel(false, ' \n ')), false);
  assert.equal(selectionHasText(null), false);
});

test('the editable selector covers text inputs, textareas and contenteditable, not buttons or checkboxes', () => {
  const parts = EDITABLE_SELECTOR.split(', ');
  assert.ok(parts.includes('textarea'));
  assert.ok(parts.some(p => p.startsWith('[contenteditable]') && p.includes(':not([contenteditable="false"])')));
  const input = parts.find(p => p.startsWith('input'));
  assert.ok(input);
  for (const type of ['button', 'submit', 'checkbox', 'radio', 'range', 'file']) {
    assert.ok(input.includes(`:not([type="${type}"])`), type);
  }
});
