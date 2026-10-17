/**
 * The webview's right-click menu (with Reload) shows in text fields only. Run with
 * Node 22.6+:
 *   node --experimental-strip-types --test app/tests/contextMenu.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EDITABLE_SELECTOR, keepsNativeContextMenu } from '../src/lib/contextMenu.ts';

/** An element whose `closest` answers for the selectors it is told it matches. */
const inside = (matches: boolean) => ({
  closest: (selector: string) => {
    assert.equal(selector, EDITABLE_SELECTOR);
    return matches ? {} : null;
  },
});

test('text fields keep the native menu, everything else loses it', () => {
  assert.equal(keepsNativeContextMenu(inside(true)), true);
  assert.equal(keepsNativeContextMenu(inside(false)), false);
  assert.equal(keepsNativeContextMenu(null), false);
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
