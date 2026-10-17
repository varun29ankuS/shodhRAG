/**
 * Esc in the PDF viewer's find field or snippet mode closes only that, not the
 * source preview around it. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/escapeScope.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { ESC_LOCAL_SELECTOR, escapeStaysLocal } from '../src/lib/escapeScope.ts';

interface Node {
  id: string;
  local: boolean;
}
const el = (id: string, local: boolean): Node & { closest(s: string): unknown } => ({
  id,
  local,
  closest: (selector: string) => {
    assert.equal(selector, ESC_LOCAL_SELECTOR);
    return local ? {} : null;
  },
});
const panel = (...inside: Node[]) => ({ contains: (n: Node | null) => n !== null && inside.includes(n) });

test('Esc in a find field or snippet mode inside the panel stays there', () => {
  const find = el('find', true);
  assert.equal(escapeStaysLocal(find, panel(find)), true);
});

test('Esc elsewhere in the panel, outside it, or with no target closes the panel', () => {
  const page = el('page', false);
  const outsideFind = el('other-find', true);
  assert.equal(escapeStaysLocal(page, panel(page)), false);
  assert.equal(escapeStaysLocal(outsideFind, panel(page)), false);
  assert.equal(escapeStaysLocal(null, panel(page)), false);
  assert.equal(escapeStaysLocal(page, null), false);
});

test('the PDF viewer marks itself while snipping and handles snip Esc before a pop-out marks it handled', () => {
  const source = readFileSync(new URL('../src/features/ask/viewer/PdfViewer.tsx', import.meta.url), 'utf8');
  assert.match(source, /data-esc-local=\{snipping \|\| marquee \? '' : undefined\}/);
  const handler = source.slice(source.indexOf('const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>)'));
  assert.ok(handler.indexOf("e.key === 'Escape' && (snipping || marquee)") < handler.indexOf('e.isDefaultPrevented()'));
  // The whole find bar closes on Esc, its buttons too (the preview leaves Esc there to it).
  const bar = source.indexOf('          role="search"');
  const findBar = source.slice(bar, source.indexOf('<label htmlFor={findInputId}', bar));
  assert.match(findBar, /if \(e\.key !== 'Escape'\) return;\s+e\.preventDefault\(\);\s+closeFind\(\);/);
});

test('Esc in the Instructions popover or on its button closes it, marked handled; an edit cancels first', () => {
  const app = readFileSync(new URL('../src/App-SplitView.tsx', import.meta.url), 'utf8');
  const start = app.indexOf('ref={instructionsButtonRef}');
  const wrapper = app.slice(app.lastIndexOf('<div', start), start);
  assert.match(wrapper, /if \(!showSystemPromptEditor \|\| e\.key !== 'Escape' \|\| e\.isDefaultPrevented\(\)\) return;\s+e\.preventDefault\(\);/);
  const popover = app.slice(start, app.indexOf('Clear all', start));
  // The edit field cancels itself and marks Esc handled before the popover sees it.
  assert.match(popover, /if \(e\.key === 'Escape'\) \{\s+\/\/[^\n]*\n\s+e\.preventDefault\(\);\s+setEditingInstructionIdx\(null\);/);
  // Focus starts inside the popover, so Esc reaches it on every webview.
  assert.match(popover, /aria-label="New instruction"[\s\S]{0,200}autoFocus/);
});
