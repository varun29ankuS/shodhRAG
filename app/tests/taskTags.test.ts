/**
 * Tag chip parsing. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/taskTags.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { MAX_TAG_LENGTH, addTags, parseTags, removeTag, sameTags, splitDraft } from '../src/features/tasks/tags.ts';

test('splits on commas, semicolons and newlines', () => {
  assert.deepEqual(parseTags('work, urgent;home\nq4'), ['work', 'urgent', 'home', 'q4']);
});

test('trims, drops # and collapses inner whitespace', () => {
  assert.deepEqual(parseTags('  #work ,##deep   focus  '), ['work', 'deep focus']);
});

test('drops empties and case-insensitive duplicates, first spelling wins', () => {
  assert.deepEqual(parseTags(',, ,Work,work,WORK,#,'), ['Work']);
  assert.deepEqual(parseTags(''), []);
});

test('truncates over-long tags', () => {
  const long = 'x'.repeat(MAX_TAG_LENGTH + 10);
  assert.deepEqual(parseTags(long), ['x'.repeat(MAX_TAG_LENGTH)]);
});

test('adding keeps existing order and skips tags already present', () => {
  assert.deepEqual(addTags(['work', 'Home'], 'home, q4, work'), ['work', 'Home', 'q4']);
  assert.deepEqual(addTags([], ' '), []);
});

test('removing is exact', () => {
  assert.deepEqual(removeTag(['a', 'b', 'A'], 'A'), ['a', 'b']);
});

test('sameTags compares order and content', () => {
  assert.equal(sameTags(['a', 'b'], ['a', 'b']), true);
  assert.equal(sameTags(['a', 'b'], ['b', 'a']), false);
  assert.equal(sameTags([], []), true);
});

test('splitDraft keeps the text after the last separator as the draft', () => {
  assert.deepEqual(splitDraft('work, home, pro'), { complete: 'work, home', draft: ' pro' });
  assert.deepEqual(splitDraft('work'), { complete: '', draft: 'work' });
  assert.deepEqual(splitDraft('work,'), { complete: 'work', draft: '' });
});
