/**
 * The code review pane (features/code/ReviewPane.tsx), as the audit's finding J asked: it
 * sits beside the conversation and never over it; the question box is always there; a
 * model that cannot review is never offered, and nothing is sent to one; and the result
 * area always says which of its states it is in. Read from the sources (the tests have no
 * DOM). Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/reviewPane.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { appDir, openingTags, source, withoutComments } from './jsxScan.ts';

const pane = withoutComments(source('features/code/ReviewPane.tsx'));

test('the pane is docked: the shell gives it its width, so it covers no view', () => {
  assert.match(pane, /useDockedWidth\(/);
  const shell = withoutComments(source('App-SplitView.tsx'));
  assert.match(shell, /const sidePaneWidth = useSidePaneWidth\(\);/);
  assert.match(shell, /paddingRight: sidePaneWidth/);
});

test('the question box is always shown, and required only for "Your question"', () => {
  assert.doesNotMatch(pane, /\{check === 'custom' && \(\s*<label/);
  assert.match(pane, /\{check === 'custom' \? 'Your question' : 'Your question \(optional\)'\}/);
  // A question typed with another check goes with it.
  const model = source('features/code/reviewModel.ts');
  assert.match(model, /question: note\(notes\.question\),/);
  assert.match(model, /if \(check === 'custom' && !base\.question\) return /);
});

test('a model that cannot review is never offered, and nothing is sent to one', () => {
  assert.match(pane, /const noReviewer = model === null;/);
  // The button is off, the "Sends … to" line is gone, and the run does nothing.
  assert.match(pane, /disabled=\{busy \|\| noReviewer\}/);
  assert.match(pane, /\{!noReviewer && \(\s*<p[^>]*>\s*\{ready \? `Sends /);
  assert.match(pane, /if \(!pane \|\| !subject \|\| busy \|\| model === null\) return;/);
  // The model is read again when it changes, so the fix shows at once.
  assert.match(pane, /window\.addEventListener\(MODEL_CHANGED_EVENT, read\);/);
  assert.match(pane, /window\.removeEventListener\(MODEL_CHANGED_EVENT, read\);/);

  // The app names the model a direct call would reach, not the one that is configured.
  const rust = readFileSync(join(appDir, 'src-tauri', 'src', 'code_review_commands.rs'), 'utf8');
  assert.match(rust, /plan_as_known\(\s*app,\s*&requested,\s*shodh_rag::harness::model_fallback::CallNeeds::direct\(""\),?\s*\)/);
  assert.match(rust, /\.find\(\|candidate\| candidate\.reachable\)/);
});

test('the result area says its state: no model, working, failed, none yet', () => {
  const at = pane.indexOf('{noReviewer ? (');
  assert.ok(at > 0);
  const states = pane.slice(at, pane.indexOf(') : null}', at));
  const order = ['kind="offline"', 'kind="loading"', 'kind="error"', 'kind="empty"'];
  let from = 0;
  for (const kind of order) {
    const next = states.indexOf(kind, from);
    assert.ok(next >= 0, kind);
    from = next;
  }
  // No model and a failed review both offer the way out.
  assert.match(states, /title="No model can review code"[\s\S]*?action=\{changeModel\}/);
  assert.match(states, /<ViewState kind="error" title="The review failed" detail=\{error\} action=\{changeModel\} \/>/);
  assert.match(pane, /<button type="button" onClick=\{requestModelPicker\}[^>]*>[\s\S]*?Switch model/);
  // What a request still needs is a hint beside the button, not a failed review.
  assert.match(pane, /setHint\(request\);/);
  assert.doesNotMatch(pane, /setError\(request\)/);
});

test('the header is in the interface type throughout', () => {
  // Both headers: the review's, and the design check's.
  const headers = [...pane.matchAll(/<header[\s\S]*?<\/header>/g)].map(m => m[0]);
  assert.equal(headers.length, 2);
  for (const header of headers) {
    for (const { tag } of openingTags(header, ['p', 'h2', 'span'])) assert.doesNotMatch(tag, /font-mono/, tag);
  }
});
