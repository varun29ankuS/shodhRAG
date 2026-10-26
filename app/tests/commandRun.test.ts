/**
 * Code-mode command run tests. Run with Node 22.6+ (no extra dependencies):
 *   node --experimental-strip-types --test app/tests/commandRun.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  MAX_COMMAND_OUTPUT_CHARS,
  MAX_PERSISTED_COMMAND_OUTPUT_CHARS,
  appendCommandOutput,
  commandElapsedMs,
  commandFailed,
  commandStateLabel,
  commandSummary,
  finishCommandRun,
  formatElapsed,
  formatTimeLimit,
  readCommandRun,
  startCommandRun,
  stopUnfinishedCommand,
  trimCommandOutput,
} from '../src/features/agent/commandRun.ts';
import type { CommandRun } from '../src/features/agent/commandRun.ts';

function started(): CommandRun {
  return startCommandRun({ commandId: 'cmd-1', command: 'cargo test', atMs: 10_000, timeLimitSecs: 600 });
}

function ended(state: CommandRun['state'], exitCode: number | null, note: string | null): CommandRun {
  return { ...started(), state, exitCode, note, durationMs: 3200 };
}

test('a started command is running with no output', () => {
  const run = started();
  assert.equal(run.state, 'running');
  assert.equal(run.output, '');
  assert.equal(run.outputCut, false);
  assert.equal(run.timeLimitSecs, 600);
  assert.equal(commandStateLabel(run), 'Running');
});

test('every state has its wording', () => {
  assert.equal(commandStateLabel(ended('done', 0, null)), 'Done (exit 0)');
  assert.equal(commandStateLabel(ended('done', 101, null)), 'Done (exit 101)');
  assert.equal(commandStateLabel(ended('done', null, null)), 'Done');
  assert.equal(commandStateLabel(ended('timed_out', null, null)), 'Timed out');
  assert.equal(commandStateLabel(ended('stopped', null, null)), 'Stopped');
  assert.equal(commandStateLabel(ended('killed', null, 'memory limit (4096 MB)')), 'Killed: memory limit (4096 MB)');
  assert.equal(commandStateLabel(ended('killed', null, null)), 'Killed: a limit was reached');
  assert.equal(
    commandStateLabel(ended('refused', null, 'Git for Windows is not installed')),
    'Refused: Git for Windows is not installed',
  );
  assert.equal(commandStateLabel(ended('refused', null, null)), 'Refused');
});

test('a finished command summarises in one line with its duration', () => {
  assert.equal(commandSummary(ended('done', 0, null)), 'Done (exit 0) · 3.2s');
  assert.equal(commandSummary(ended('killed', null, 'CPU time limit')), 'Killed: CPU time limit · 3.2s');
  assert.equal(commandSummary({ ...ended('timed_out', null, null), durationMs: 600_000 }), 'Timed out · 10m 00s');
  // A refused command never ran, so it has no duration.
  assert.equal(commandSummary({ ...ended('refused', null, 'No shell'), durationMs: 0 }), 'Refused: No shell');
  // A command stopped when the app closed has no known duration.
  assert.equal(commandSummary({ ...ended('stopped', null, null), durationMs: null }), 'Stopped');
});

test('failures are a nonzero exit, a timeout, a kill or a refusal; a stop is not', () => {
  assert.equal(commandFailed(ended('done', 0, null)), false);
  assert.equal(commandFailed(ended('done', null, null)), false);
  assert.equal(commandFailed(ended('done', 1, null)), true);
  assert.equal(commandFailed(ended('timed_out', null, null)), true);
  assert.equal(commandFailed(ended('killed', null, null)), true);
  assert.equal(commandFailed(ended('refused', null, null)), true);
  assert.equal(commandFailed(ended('stopped', null, null)), false);
  assert.equal(commandFailed(started()), false);
});

test('output is appended in order', () => {
  let run = started();
  run = appendCommandOutput(run, 'compiling\n');
  run = appendCommandOutput(run, 'error: x\n');
  run = appendCommandOutput(run, '');
  assert.equal(run.output, 'compiling\nerror: x\n');
  assert.equal(run.outputCut, false);
});

test('output keeps only its tail and says earlier output was dropped', () => {
  let run = appendCommandOutput(started(), 'a'.repeat(MAX_COMMAND_OUTPUT_CHARS - 2));
  assert.equal(run.outputCut, false);
  run = appendCommandOutput(run, 'bcde');
  assert.equal(run.output.length, MAX_COMMAND_OUTPUT_CHARS);
  assert.ok(run.output.endsWith('aabcde'));
  assert.equal(run.outputCut, true);
  // Once cut, it stays marked even when later text fits.
  run = appendCommandOutput(run, 'f');
  assert.equal(run.outputCut, true);
  assert.ok(run.output.endsWith('bcdef'));
});

test('one chunk longer than the cap keeps its own tail', () => {
  const chunk = `${'x'.repeat(MAX_COMMAND_OUTPUT_CHARS)}END`;
  const run = appendCommandOutput(started(), chunk);
  assert.equal(run.output.length, MAX_COMMAND_OUTPUT_CHARS);
  assert.ok(run.output.endsWith('END'));
  assert.equal(run.outputCut, true);
});

test('the tail never starts inside a surrogate pair', () => {
  // 'ab😀cd' is 6 UTF-16 units; a 3-unit tail would start on the emoji's second half.
  const run = appendCommandOutput(started(), 'ab😀cd', 3);
  assert.equal(run.output, 'cd');
  assert.equal(run.outputCut, true);
  assert.equal(appendCommandOutput(started(), 'ab😀cd', 4).output, '😀cd');
});

test('trimming for storage keeps the tail and marks the cut; a short output is left alone', () => {
  const short = appendCommandOutput(started(), 'ok\n');
  assert.equal(trimCommandOutput(short, MAX_PERSISTED_COMMAND_OUTPUT_CHARS), short);
  const long = appendCommandOutput(started(), `${'y'.repeat(MAX_PERSISTED_COMMAND_OUTPUT_CHARS)}tail`);
  const trimmed = trimCommandOutput(long, MAX_PERSISTED_COMMAND_OUTPUT_CHARS);
  assert.equal(trimmed.output.length, MAX_PERSISTED_COMMAND_OUTPUT_CHARS);
  assert.ok(trimmed.output.endsWith('tail'));
  assert.equal(trimmed.outputCut, true);
});

test('finishing records the state, exit code, note and duration', () => {
  const run = finishCommandRun(appendCommandOutput(started(), 'ok\n'), {
    state: 'done', exitCode: 0, note: '2 background processes were stopped when the command ended.', durationMs: 4100,
  });
  assert.equal(run.state, 'done');
  assert.equal(run.exitCode, 0);
  assert.equal(run.note, '2 background processes were stopped when the command ended.');
  assert.equal(run.durationMs, 4100);
  assert.equal(run.output, 'ok\n');
});

test('an unfinished command is stopped with a note; a finished one is left alone', () => {
  const stopped = stopUnfinishedCommand(started(), 'Ended with the answer', 15_500);
  assert.equal(stopped.state, 'stopped');
  assert.equal(stopped.note, 'Ended with the answer');
  assert.equal(stopped.durationMs, 5500);
  assert.equal(stopUnfinishedCommand(started(), 'x', null).durationMs, null);
  assert.equal(stopUnfinishedCommand(started(), 'x', 0).durationMs, 0);
  const done = ended('done', 0, null);
  assert.equal(stopUnfinishedCommand(done, 'x', 99_999), done);
});

test('elapsed time is whole seconds, then minutes, then hours', () => {
  assert.equal(formatElapsed(0), '0s');
  assert.equal(formatElapsed(-5), '0s');
  assert.equal(formatElapsed(Number.NaN), '0s');
  assert.equal(formatElapsed(999), '0s');
  assert.equal(formatElapsed(12_400), '12s');
  assert.equal(formatElapsed(59_999), '59s');
  assert.equal(formatElapsed(60_000), '1m 00s');
  assert.equal(formatElapsed(65_000), '1m 05s');
  assert.equal(formatElapsed(3_599_000), '59m 59s');
  assert.equal(formatElapsed(3_600_000), '1h 00m 00s');
  assert.equal(commandElapsedMs(started(), 22_000), 12_000);
  // A clock behind the start never shows a negative time.
  assert.equal(commandElapsedMs(started(), 9_000), 0);
});

test('time limits read in words', () => {
  assert.equal(formatTimeLimit(45), '45s');
  assert.equal(formatTimeLimit(600), '10 min');
  assert.equal(formatTimeLimit(3600), '1 h');
  assert.equal(formatTimeLimit(150), '2m 30s');
});

test('a stored command is validated and capped', () => {
  const stored = JSON.parse(JSON.stringify(ended('done', 0, 'n')));
  assert.deepEqual(readCommandRun(stored), ended('done', 0, 'n'));
  assert.equal(readCommandRun(null), null);
  assert.equal(readCommandRun({ ...stored, state: 'exploded' }), null);
  assert.equal(readCommandRun({ ...stored, commandId: 3 }), null);
  const big = readCommandRun({ ...stored, output: 'z'.repeat(MAX_COMMAND_OUTPUT_CHARS + 10) });
  assert.equal(big?.output.length, MAX_COMMAND_OUTPUT_CHARS);
  assert.equal(big?.outputCut, true);
  const loose = readCommandRun({ ...stored, exitCode: 'zero', note: '', output: 7 });
  assert.equal(loose?.exitCode, null);
  assert.equal(loose?.note, null);
  assert.equal(loose?.output, '');
});
