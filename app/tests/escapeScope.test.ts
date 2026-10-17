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
