/**
 * The code review pane's rules (`features/code/reviewModel.ts`): what the reviewer is
 * given from a selection, how a finding is marked once its check ran ("Verified by
 * running" or "From reading only"), the messages a check and a fix send to the Code
 * conversation, and when they cannot be sent. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/codeReview.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  applyFixMessage,
  checkMessage,
  citationText,
  conversationProblem,
  fenceFor,
  piecesAsChanges,
  piecesFromChanges,
  reviewedSelection,
  reviewedWords,
  verification,
  withOutput,
} from '../src/features/code/reviewModel.ts';
import type { CheckRun, Finding, ReviewRequest, VerifyStep } from '../src/features/code/reviewModel.ts';
import { languageOf } from '../src/features/code/diffModel.ts';
import type { FileChange } from '../src/features/code/diffModel.ts';

const change: FileChange = {
  path: 'src/sum.rs',
  status: 'changed',
  binary: false,
  tooLarge: false,
  hunks: [
    {
      lines: [
        { kind: 'same', text: 'fn sum() {', old: 1, new: 1 },
        { kind: 'removed', text: '  for i in 0..=n {', old: 2, new: null },
        { kind: 'added', text: '  for i in 0..n {', old: null, new: 2 },
        { kind: 'same', text: '}', old: 3, new: 3 },
      ],
    },
  ],
};

const finding: Finding = {
  severity: 'critical',
  title: 'reads past the end',
  citation: { path: 'src/sum.rs', line: 2, endLine: 2 },
  scenario: 'sum(&[1]) panics',
  fix: 'use 0..n',
  replace: { path: 'src/sum.rs', old: '0..=n', new: '0..n' },
  edges: [],
  verify: { command: 'cargo test -p demo sum', expect: 'fails', outputContains: 'index out of bounds' },
};

const ran = (over: Partial<CheckRun>): CheckRun => ({
  command: 'cargo test -p demo sum',
  commandId: 'c1',
  state: 'done',
  exitCode: 101,
  output: '',
  note: null,
  ...over,
});

test('the reviewer gets the selected lines of the selected file, each under its own number', () => {
  const other: FileChange = { ...change, path: 'src/other.rs' };
  const all = piecesFromChanges([change, other], null, languageOf);
  assert.equal(all.length, 2);
  const chosen = piecesFromChanges([change, other], { path: 'src/sum.rs', from: 2, to: 2 }, languageOf);
  assert.equal(chosen.length, 1);
  assert.equal(chosen[0].language, 'rust');
  assert.deepEqual(chosen[0].lines, [
    { number: 2, kind: 'removed', text: '  for i in 0..=n {' },
    { number: 2, kind: 'added', text: '  for i in 0..n {' },
  ]);
  const request: ReviewRequest = { check: 'algorithm', pieces: chosen, question: null, folder: null, flows: null, context: null, canVerify: false };
  assert.equal(reviewedWords(request), '2 lines of src/sum.rs');
  assert.deepEqual(reviewedSelection(request), { path: 'src/sum.rs', from: 2, to: 2 });
  // Shown back in the pane as the diff it was.
  const back = piecesAsChanges(chosen);
  assert.deepEqual(back[0].hunks[0].lines.map(l => [l.kind, l.old, l.new]), [
    ['removed', 2, null],
    ['added', null, 2],
  ]);
  assert.equal(reviewedWords({ ...request, pieces: [], folder: 'crates' }), 'the structure of crates');
});

test('a finding is from reading only until its check ran to its end', () => {
  const step = finding.verify as VerifyStep;
  assert.equal(verification(step, null).label, 'From reading only');
  assert.equal(verification(null, ran({})).status, 'reading');
  assert.equal(verification(step, ran({ state: 'waiting', commandId: null })).status, 'pending');
  assert.equal(verification(step, ran({ state: 'running' })).status, 'pending');
  const refused = verification(step, ran({ state: 'refused', exitCode: null, note: 'you declined it' }));
  assert.equal(refused.status, 'not_run');
  assert.match(refused.label, /^From reading only: the check did not run \(you declined it\)$/);
});

test('a check that ran is verified by running, with what it showed', () => {
  const step = finding.verify as VerifyStep;
  const confirmed = verification(step, ran({ output: "thread panicked: index out of bounds: the len is 1" }));
  assert.equal(confirmed.status, 'confirmed');
  assert.match(confirmed.label, /ran cargo test -p demo sum, observed exit 101, output shows “index out of bounds”/);
  const passed = verification(step, ran({ exitCode: 0, output: 'test result: ok' }));
  assert.equal(passed.status, 'not_confirmed');
  assert.match(passed.label, /does not confirm/);
  const otherFailure = verification(step, ran({ output: 'error[E0425]: cannot find value' }));
  assert.equal(otherFailure.status, 'not_confirmed', 'a failure without the expected output does not confirm');
  const expectsPass = verification({ command: 'x', expect: 'passes', outputContains: null }, ran({ command: 'x', exitCode: 0 }));
  assert.equal(expectsPass.status, 'confirmed');
});

test('a check keeps the end of a long output', () => {
  let run = ran({ output: '' });
  run = withOutput(run, 'a'.repeat(30_000));
  run = withOutput(run, 'END');
  assert.equal(run.output.length, 20_000);
  assert.ok(run.output.endsWith('END'));
});

test('a check and a fix are sent as exact text the content cannot break out of', () => {
  const message = checkMessage(finding, finding.verify as VerifyStep);
  assert.match(message, /run_command, exactly as written/);
  assert.ok(message.includes('```bash\ncargo test -p demo sum\n```'));
  assert.ok(message.includes(`at ${citationText(finding.citation!)}`));
  const tricky = { ...finding, replace: { path: 'a.md', old: 'x ``` y', new: '````' } };
  const fix = applyFixMessage(tricky, tricky.replace);
  assert.ok(fix.includes('````\nx ``` y\n````'), fix);
  assert.ok(fix.includes('`````\n````\n`````'), fix);
  assert.match(fix, /one edit of a\.md with the edit tool/);
  assert.equal(fenceFor('no ticks'), '```');
});

test('checks and fixes go only to a Code conversation of the project that can run them', () => {
  const code = { mode: 'code' as const, agentMode: 'ask', workspaceId: 'ws-1', running: false };
  assert.equal(conversationProblem(code, 'ws-1'), null);
  assert.equal(conversationProblem(code, null), null);
  assert.match(conversationProblem(null, 'ws-1') ?? '', /Code conversation/);
  assert.match(conversationProblem({ ...code, mode: 'research' }, 'ws-1') ?? '', /Code conversation/);
  assert.match(conversationProblem({ ...code, workspaceId: 'ws-2' }, 'ws-1') ?? '', /another project/);
  assert.match(conversationProblem({ ...code, agentMode: 'plan' }, 'ws-1') ?? '', /Plan mode only reads/);
  assert.match(conversationProblem({ ...code, running: true }, 'ws-1') ?? '', /Wait for it/);
});
