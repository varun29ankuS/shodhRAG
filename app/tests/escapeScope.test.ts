/**
 * Esc in the PDF viewer's find field or snippet mode closes only that, not the
 * source preview around it. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/escapeScope.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { ESC_LOCAL_SELECTOR, escapeStaysLocal, insideOtherDialog, instructionsEscape } from '../src/lib/escapeScope.ts';

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

const esc = (over: Partial<Parameters<typeof instructionsEscape>[0]> = {}) =>
  instructionsEscape({ key: 'Escape', handled: false, editing: false, inOtherDialog: false, ...over });

test('Esc while the Instructions popover is open cancels an edit first, then closes it', () => {
  assert.equal(esc({ editing: true }), 'cancel-edit');
  assert.equal(esc(), 'close');
});

test('the popover leaves other keys, handled keys and keys of a dialog on top alone', () => {
  assert.equal(esc({ key: 'Enter' }), 'ignore');
  assert.equal(esc({ handled: true }), 'ignore');
  assert.equal(esc({ inOtherDialog: true, editing: true }), 'ignore');
});

/** A target inside (or outside) a `[role="dialog"]`, for `insideOtherDialog`. */
const inDialog = (id: string, dialog: boolean) => ({
  id,
  local: false,
  closest: (selector: string) => {
    assert.equal(selector, '[role="dialog"]');
    return dialog ? {} : null;
  },
});

test('Esc in a dialog over the preview belongs to that dialog, not the preview', () => {
  const feedbackField = inDialog('feedback', true);
  const previewButton = inDialog('preview', true);
  const page = inDialog('page', false);
  const preview = panel(previewButton);
  assert.equal(insideOtherDialog(feedbackField, preview), true);
  assert.equal(insideOtherDialog(previewButton, preview), false, 'the preview is a dialog itself');
  assert.equal(insideOtherDialog(page, preview), false);
  assert.equal(insideOtherDialog(null, preview), false);
  assert.equal(insideOtherDialog(feedbackField, null), true);
});
