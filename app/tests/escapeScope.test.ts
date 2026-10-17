/**
 * Esc in the PDF viewer's find field or snippet mode closes only that, not the
 * source preview around it. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/escapeScope.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { ESC_LOCAL_SELECTOR, escapeStaysLocal, insideOtherDialog, instructionsEscape, leavesSnippetMode } from '../src/lib/escapeScope.ts';

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

test('Esc leaves snippet mode while snipping, but not from the find bar, which closes itself', () => {
  assert.equal(leavesSnippetMode({ key: 'Escape', snipping: true, inFindBar: false }), true);
  assert.equal(leavesSnippetMode({ key: 'Escape', snipping: true, inFindBar: true }), false);
  assert.equal(leavesSnippetMode({ key: 'Escape', snipping: false, inFindBar: false }), false);
  assert.equal(leavesSnippetMode({ key: 's', snipping: true, inFindBar: false }), false);
});

test('a snipping viewer is data-esc-local, so the preview leaves its Esc to it', () => {
  // The viewer root while snipping: matches ESC_LOCAL_SELECTOR inside the preview panel.
  const snippingViewer = el('viewer', true);
  assert.equal(escapeStaysLocal(snippingViewer, panel(snippingViewer)), true);
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
