/**
 * The question dots rail (features/ask/questionDots.ts, QuestionRail.tsx): which messages
 * get a dot, when the rail shows, which dot is filled, and the label each dot shows.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/questionDots.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  RAIL_LABEL_CHARS,
  RAIL_MIN_QUESTIONS,
  currentQuestion,
  firstLine,
  keepInView,
  railQuestions,
  railShown,
} from '../src/features/ask/questionDots.ts';

const source = (path: string) => readFileSync(new URL(`../src/${path}`, import.meta.url), 'utf8');

const conversation = [
  { role: 'user', content: 'First question' },
  { role: 'assistant', content: 'An answer' },
  { role: 'system', content: 'A notice' },
  { role: 'user', content: '\n\n  Second   question\nwith a second line' },
  { role: 'assistant', content: 'Another answer' },
  { role: 'user', content: 'Third' },
  { role: 'assistant', content: 'Last answer' },
];

test('one dot per question, at its place in the conversation', () => {
  assert.deepEqual(railQuestions(conversation), [
    { index: 0, label: 'First question' },
    { index: 3, label: 'Second question' },
    { index: 5, label: 'Third' },
  ]);
  // A question of only an image still gets a dot, and a name.
  assert.deepEqual(railQuestions([{ role: 'user', content: '' }]), [{ index: 0, label: 'Question' }]);
});

test('the rail shows from three questions', () => {
  assert.equal(RAIL_MIN_QUESTIONS, 3);
  assert.ok(!railShown(0));
  assert.ok(!railShown(2));
  assert.ok(railShown(3));
  assert.ok(railShown(40));
  const ask = source('features/ask/AskView.tsx');
  assert.match(ask, /\{railShown\(questions\.length\) && \(\s*<QuestionRail/);
});

test('the filled dot is the question whose turn is in view', () => {
  const questions = railQuestions(conversation);
  assert.equal(currentQuestion([], 4), -1);
  // The question itself, or its answer, at the top of the view.
  assert.equal(currentQuestion(questions, 0), 0);
  assert.equal(currentQuestion(questions, 2), 0);
  assert.equal(currentQuestion(questions, 3), 1);
  assert.equal(currentQuestion(questions, 4), 1);
  assert.equal(currentQuestion(questions, 6), 2);
  // Past the end, the last; before a first question (a notice first), the first.
  assert.equal(currentQuestion(questions, 99), 2);
  assert.equal(currentQuestion([{ index: 2, label: 'a' }, { index: 4, label: 'b' }], 0), 0);
});

test('a dot shows the question\'s first line, whole on screen', () => {
  assert.equal(firstLine('  One   line  '), 'One line');
  assert.equal(firstLine('\n\nAfter blanks\nnext'), 'After blanks');
  const long = 'word '.repeat(40).trim();
  const cut = firstLine(long);
  assert.ok(cut.length <= RAIL_LABEL_CHARS, `${cut.length}`);
  assert.ok(cut.endsWith('…'));
  // Cut at a word.
  assert.ok(/word…$/.test(cut));
  // One long word is cut where it must be.
  const word = 'x'.repeat(200);
  assert.equal(firstLine(word).length, RAIL_LABEL_CHARS);
  // The label is cut in code, never by the stylesheet (the launch test measures it).
  const rail = source('features/ask/QuestionRail.tsx');
  assert.doesNotMatch(rail, /\btruncate\b|line-clamp|text-ellipsis/);
});

test('the filled dot stays in view of a long rail', () => {
  // Already in view: unchanged.
  assert.equal(keepInView(0, 240, 48, 24), 0);
  // Above: scrolled up to it; below: scrolled until its bottom shows.
  assert.equal(keepInView(100, 240, 48, 24), 48);
  assert.equal(keepInView(0, 240, 480, 24), 264);
});

test('the rail sits in the gutter wide blocks leave, below find and above the message box', () => {
  const ask = source('features/ask/AskView.tsx');
  assert.match(ask, /insetInlineEnd: hostBox\.scrollbar \+ \(COLUMN_GUTTER_PX - RAIL_WIDTH_PX\) \/ 2,/);
  assert.match(ask, /top: RAIL_TOP_PX,\s*bottom: composerSpace,/);
  // Its dots are the width it is given, and a 24px target.
  assert.match(ask, /const RAIL_WIDTH_PX = 24;/);
  const rail = source('features/ask/QuestionRail.tsx');
  assert.match(rail, /'group w-6 h-6 rounded-full/);
  assert.match(rail, /const DOT_PX = 24;/);
  // A click jumps through the conversation's own reveal.
  assert.match(ask, /const jumpToQuestion = useCallback\(\(index: number\) => \{\s*void revealMessage\(index\);/);
});
