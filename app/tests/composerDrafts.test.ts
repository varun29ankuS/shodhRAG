/**
 * The unsent question survives view switches and is shared by Ask and the dock;
 * dock replies use the same send options (Library source choices) as Ask. Run
 * with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/composerDrafts.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { DraftStore } from '../src/features/ask/draftStore.ts';
import { sendOptionsFor, sendOptionsKey } from '../src/features/ask/sendOptions.ts';

test('a draft is kept per conversation and outlives the component that typed it', () => {
  const drafts = new DraftStore();
  drafts.set('c1', 'What does figure 3');
  drafts.set('c2', 'Compare');
  assert.equal(drafts.get('c1'), 'What does figure 3');
  assert.equal(drafts.get('c2'), 'Compare');
  assert.equal(drafts.get('c3'), '');
  assert.equal(drafts.get(null), '');
});

test('appending a snippet builds on the current text; clearing forgets it', () => {
  const drafts = new DraftStore();
  drafts.set('c1', 'Explain');
  drafts.update('c1', current => `${current} this`);
  assert.equal(drafts.get('c1'), 'Explain this');
  drafts.set('c1', '');
  assert.equal(drafts.get('c1'), '');
});

test('subscribers hear real changes only, and stop hearing once unsubscribed', () => {
  const drafts = new DraftStore();
  let calls = 0;
  const stop = drafts.subscribe(() => { calls += 1; });
  drafts.set('c1', 'a');
  drafts.set('c1', 'a');
  drafts.update('c1', t => `${t}b`);
  assert.equal(calls, 2);
  stop();
  drafts.set('c1', 'abc');
  assert.equal(calls, 2);
});

test('Ask and the dock read one draft store and one send-options function', () => {
  for (const file of ['../src/features/ask/AskView.tsx', '../src/components/shell/ConversationDock.tsx']) {
    const source = readFileSync(new URL(file, import.meta.url), 'utf8');
    assert.match(source, /useDraft\(session\.activeConversationId\)/, file);
    assert.match(source, /useSendOptions\(sources\)/, file);
    assert.doesNotMatch(source, /useState\(''\)/, file);
  }
});

const src = (id: string, selected: boolean) => ({ id, name: `Folder ${id}`, selected });

test('only some sources included: answers search just those', () => {
  const options = sendOptionsFor([src('a', true), src('b', false), src('c', true)]);
  assert.deepEqual(options, { spaceId: 'a', spaceName: 'Folder a', sourceIds: ['a', 'c'] });
});

test('every source or none included: answers search everything', () => {
  assert.deepEqual(sendOptionsFor([src('a', true), src('b', true)]).sourceIds, []);
  assert.deepEqual(sendOptionsFor([src('a', false)]), { spaceId: null, spaceName: null, sourceIds: [] });
  assert.deepEqual(sendOptionsFor([]).sourceIds, []);
});

test('equal options share a key, different choices do not', () => {
  const one = sendOptionsFor([src('a', true), src('b', false)]);
  const same = sendOptionsFor([{ ...src('a', true) }, { ...src('b', false) }]);
  const other = sendOptionsFor([src('a', false), src('b', true)]);
  assert.equal(sendOptionsKey(one), sendOptionsKey(same));
  assert.notEqual(sendOptionsKey(one), sendOptionsKey(other));
});
