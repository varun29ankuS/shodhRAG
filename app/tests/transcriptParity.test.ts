/**
 * Transcript parity: the Ask view's reducer and the backend's (`app/src-tauri/src/agent_transcript.rs`,
 * which saves answers without a window) fold the same fixtures to the same stored transcript.
 * Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/transcriptParity.test.ts
 *
 * Each fixture in `fixtures/transcripts` is a synthetic session (no user data): the
 * transcript the answer was first saved with (`seed`), the events and local actions of its
 * run, and the stored transcript and answer text both reducers must give. The Rust test
 * `fixtures_reduce_to_the_saved_transcript` reads the same files.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { answerText, reduceAll, toCheckpoint, toPersisted } from '../src/features/agent/reducer.ts';
import type { TranscriptAction, TranscriptState } from '../src/features/agent/reducer.ts';
import { startsCheck } from '../src/features/ask/chatPersistence.ts';

interface Fixture {
  name: string;
  seed: TranscriptState;
  actions: TranscriptAction[];
  expected: unknown;
  expectedText: string;
  checkpointAfter?: number;
  expectedCheckpoint?: unknown;
}

const DIR = fileURLToPath(new URL('./fixtures/transcripts/', import.meta.url));

function fixtures(): Array<[string, Fixture]> {
  return readdirSync(DIR)
    .filter(name => name.endsWith('.json'))
    .sort()
    .map(name => [name, JSON.parse(readFileSync(join(DIR, name), 'utf8')) as Fixture]);
}

/** The stored form, as it is written to the conversations file. */
function stored(state: TranscriptState): unknown {
  return JSON.parse(JSON.stringify(state));
}

/** The seed as the live answer starts from it (a fresh copy per fold). */
function seedOf(fixture: Fixture): TranscriptState {
  return structuredClone(fixture.seed);
}

test('fixtures_reduce_to_the_saved_transcript', () => {
  const all = fixtures();
  assert.ok(all.length >= 5, 'every fixture is read');
  for (const [name, fixture] of all) {
    const state = reduceAll(seedOf(fixture), fixture.actions);
    assert.deepEqual(stored(toPersisted(state)), fixture.expected, `${name}: the transcript differs`);
    assert.equal(answerText(state), fixture.expectedText, `${name}: the answer text differs`);
  }
});

test('a checkpoint is the transcript at the start of the check', () => {
  let checked = 0;
  for (const [name, fixture] of fixtures()) {
    if (fixture.checkpointAfter === undefined) continue;
    checked += 1;
    const done = fixture.actions.slice(0, fixture.checkpointAfter);
    const state = reduceAll(seedOf(fixture), done);
    assert.ok(startsCheck(done.slice(-1), answerText(state)), `${name}: the check starts there`);
    assert.deepEqual(stored(toCheckpoint(state)), fixture.expectedCheckpoint, `${name}: the checkpoint differs`);
  }
  assert.ok(checked > 0, 'a fixture has a checkpoint');
});

test('fixtures hold synthetic sessions only', () => {
  for (const [name, fixture] of fixtures()) {
    const text = JSON.stringify(fixture);
    // No real paths of a user's machine, and no e-mail addresses.
    assert.doesNotMatch(text, /[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/, `${name}: an e-mail address`);
    assert.doesNotMatch(text, /Users[\\/]+[^\\/"]+[\\/]/, `${name}: a user folder`);
  }
});
