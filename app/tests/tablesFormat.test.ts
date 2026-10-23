/**
 * Evidence table wording tests: every code the Rust side sends has words of its own
 * (checked against the Rust sources, so a new code cannot slip through as the generic
 * text), the validate reasons mirror `validate::reason`, pause reasons, the reset in local
 * time, and what Fill, Cancel and a fill by hand did. Run with Node 22.6+:
 *   node --experimental-strip-types --test app/tests/tablesFormat.test.ts
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import {
  callsLine,
  cancelOutcome,
  cellExplanation,
  cancelPending,
  capNotice,
  estimateText,
  failedCellsSince,
  fillStartOutcome,
  fillStatusText,
  GENERIC_REASON,
  handFillOutcome,
  jobActive,
  knownReason,
  localTime,
  nextUtcMidnight,
  NOT_FOUND_TEXT,
  pauseText,
  REASON_CODES,
  reasonText,
  splitError,
} from '../src/features/tables/format.ts';
import type { Cell, FillStatus, HandFill, JobState, JobView, PauseReason } from '../src/features/tables/types.ts';

const TABLES_DIR = new URL('../../crates/shodh-rag/src/research/tables/', import.meta.url);
const read = (url: URL) => readFileSync(url, 'utf8');
const appSource = (name: string) => read(new URL(`../src-tauri/src/${name}`, import.meta.url));

/** The text of `source` from `start` up to the next `end` after it. */
function block(source: string, start: string, end: string): string {
  const from = source.indexOf(start);
  assert.ok(from >= 0, `found ${start}`);
  const to = source.indexOf(end, from + start.length);
  assert.ok(to > from, `found ${end} after ${start}`);
  return source.slice(from, to);
}

function strings(text: string, pattern: RegExp): string[] {
  return [...text.matchAll(pattern)].map(m => m[1]);
}

/** Every code string the tables backend can send to the interface. */
function rustCodes(): Map<string, string> {
  const codes = new Map<string, string>();
  const add = (where: string, list: string[]) => {
    assert.ok(list.length > 0, `codes found in ${where}`);
    for (const code of list) codes.set(code, where);
  };
  for (const file of readdirSync(TABLES_DIR).filter(f => f.endsWith('.rs') && !f.endsWith('_tests.rs') && f !== 'testkit.rs')) {
    const found = strings(read(new URL(file, TABLES_DIR)), /pub const [A-Z_]+: &str = "([a-z_]+)";/g);
    if (found.length > 0) add(`research/tables/${file}`, found);
  }
  const commands = appSource('table_commands.rs');
  add('table_commands.rs coded()', strings(commands, /coded\(\s*"([a-z_]+)"/g));
  add('table_commands.rs model_error', strings(block(commands, 'pub fn model_error', 'coded(code'), /"([a-z_]+)"/g));
  add('table_commands.rs halt_code', strings(block(commands, 'fn halt_code', '\n}\n'), /"([a-z_]+)"/g));
  const learn = appSource('memory_learn.rs');
  add('memory_learn.rs TableModelError::code', strings(block(learn, 'impl TableModelError', 'pub fn message'), /=> "([a-z_]+)"/g));
  const research = appSource('research_commands.rs');
  add('research_commands.rs From<ResearchError>', strings(block(research, 'impl From<ResearchError> for ResearchCommandError', 'message: error.to_string()'),/=> "([a-z_]+)"/g));
  add('research_commands.rs code:', strings(research, /code: "([a-z_]+)"/g));
  return codes;
}

test('every code the tables backend sends has words of its own', () => {
  const codes = rustCodes();
  for (const required of ['busy', 'model_refused', 'model_unavailable', 'diagnostics_off', 'hand_fill_running', 'fill_running', 'value_no_longer_fits', 'not_found_partial', 'refused_reasoning', 'local_only', 'no_key', 'no_model', 'refill_failed', 'daily_cap']) {
    assert.ok(codes.has(required), `the Rust sources still send ${required}`);
  }
  for (const [code, where] of codes) {
    assert.ok(knownReason(code), `${code} (${where}) is listed in REASON_CODES`);
    const text = reasonText(code);
    assert.notEqual(text, GENERIC_REASON, `${code} has its own words`);
    assert.ok(text.trim().length > 0);
  }
});

test('the validate reasons are worded as validate::reason words them', () => {
  const validate = read(new URL('validate.rs', TABLES_DIR));
  const consts = new Map<string, string>();
  for (const m of validate.matchAll(/pub const ([A-Z_]+): &str = "([a-z_]+)";/g)) consts.set(m[1], m[2]);
  const reasons = block(validate, 'pub fn reason(code: &str)', '_ =>');
  let mirrored = 0;
  for (const m of reasons.matchAll(/^\s*([A-Z_]+) => "([^"]+)",$/gm)) {
    const code = consts.get(m[1]);
    assert.ok(code, `${m[1]} is a validate code`);
    assert.equal(reasonText(code), m[2], code);
    mirrored += 1;
  }
  assert.ok(mirrored >= 12, `mirrored ${mirrored} reasons`);
});

test('every listed code is a code, and unknown codes get the generic, non-empty text', () => {
  assert.ok(REASON_CODES.length >= 40);
  for (const code of REASON_CODES) assert.match(code, /^[a-z_]+$/);
  assert.ok(GENERIC_REASON.trim().length > 0);
  assert.equal(reasonText('a_code_from_the_future'), GENERIC_REASON);
  assert.equal(reasonText(''), GENERIC_REASON);
  assert.equal(reasonText(null), GENERIC_REASON);
  assert.equal(knownReason('a_code_from_the_future'), false);
});

test('stored errors are read apart: a kept result whose refresh failed, and a code with detail', () => {
  assert.deepEqual(splitError('refill_failed:model_timeout'), { code: 'model_timeout', detail: null, refillFailed: true });
  assert.deepEqual(splitError('search_failed: no passage of the paper matched any column'), {
    code: 'search_failed',
    detail: 'no passage of the paper matched any column',
    refillFailed: false,
  });
  assert.deepEqual(splitError('refill_failed:search_failed: models missing'), { code: 'search_failed', detail: 'models missing', refillFailed: true });
  assert.deepEqual(splitError('quote_too_short'), { code: 'quote_too_short', detail: null, refillFailed: false });
  assert.equal(reasonText('refill_failed:model_timeout'), "Couldn't refresh: The model could not be reached.");
  assert.equal(reasonText('refill_failed:unknown'), `Couldn't refresh: ${GENERIC_REASON}`);
  assert.equal(reasonText('search_failed: embedder missing'), reasonText('search_failed'));
});

test('not found is always "Not found in the passages searched", never "not mentioned"', () => {
  assert.equal(NOT_FOUND_TEXT, 'Not found in the passages searched');
  for (const code of REASON_CODES) assert.doesNotMatch(reasonText(code), /not mentioned/i, code);
});

test('a refused model says what to do', () => {
  assert.equal(reasonText('refused_reasoning'), "This model always thinks at length and can't be capped; pick another model for tables (Settings → Model).");
  assert.match(reasonText('model_refused'), /can't be capped; pick another/);
  assert.match(reasonText('model_unavailable'), /Settings → Model/);
  assert.match(reasonText('diagnostics_off'), /Settings → Tables/);
});

// ── Time ────────────────────────────────────────────────────────────

const INDIA = { locale: 'en-GB', timeZone: 'Asia/Kolkata' };
const NEW_YORK = { locale: 'en-GB', timeZone: 'America/New_York' };
const RESET = Date.UTC(2026, 9, 24, 0, 0); // the next 00:00 UTC

test('the reset is shown in local time, relative to today', () => {
  // 15:30 in India: the UTC midnight is 05:30 tomorrow there.
  assert.match(localTime(RESET, Date.UTC(2026, 9, 23, 10, 0), INDIA), /^0?5:30 tomorrow$/);
  // 01:30 in India, already the 24th: the reset is today.
  assert.match(localTime(RESET, Date.UTC(2026, 9, 23, 20, 0), INDIA), /^0?5:30$/);
  // In New York the UTC midnight is 20:00 the same evening.
  assert.equal(localTime(RESET, Date.UTC(2026, 9, 23, 10, 0), NEW_YORK), '20:00');
  assert.match(localTime(RESET, Date.UTC(2026, 9, 20, 10, 0), INDIA), /^0?5:30 on Sat 24 Oct$/);
});

test('"tomorrow" follows the calendar across a DST change', () => {
  // New York leaves DST at 02:00 on Sun 1 Nov 2026, a 25-hour day. At 00:30 that morning,
  // 24 hours on is still 1 Nov (23:30), yet 00:00 on 2 Nov is tomorrow.
  const now = Date.UTC(2026, 10, 1, 4, 30); // 00:30 EDT, 1 Nov
  const midnight = Date.UTC(2026, 10, 2, 5, 0); // 00:00 EST, 2 Nov
  assert.match(localTime(midnight, now, NEW_YORK), /^0?0:00 tomorrow$/);
  // And on the spring change (Sun 8 Mar 2026, a 23-hour day), the day after next is not tomorrow.
  const spring = Date.UTC(2026, 2, 8, 5, 30); // 00:30 EST, 8 Mar
  assert.match(localTime(Date.UTC(2026, 2, 9, 4, 30), spring, NEW_YORK), /^0?0:30 tomorrow$/);
  assert.match(localTime(Date.UTC(2026, 2, 10, 4, 30), spring, NEW_YORK), /^0?0:30 on Tue 10 Mar$/);
});

test('the next UTC midnight is the day count reset', () => {
  assert.equal(nextUtcMidnight(Date.UTC(2026, 9, 23, 10, 0)), RESET);
  assert.equal(nextUtcMidnight(Date.UTC(2026, 9, 23, 23, 59, 59, 999)), RESET);
  assert.equal(nextUtcMidnight(Date.UTC(2026, 9, 24, 0, 0)), Date.UTC(2026, 9, 25, 0, 0));
  assert.equal(nextUtcMidnight(Date.UTC(2026, 11, 31, 12, 0)), Date.UTC(2027, 0, 1, 0, 0));
});

test('the day\'s calls line names the count, the cap and the local reset', () => {
  assert.match(
    callsLine(12, 30, RESET, Date.UTC(2026, 9, 23, 10, 0), INDIA),
    /^Tables have made 12 of 30 calls today; resets at 0?5:30 tomorrow\.$/,
  );
});

// ── Status and pauses ───────────────────────────────────────────────

const NOW = Date.UTC(2026, 9, 23, 10, 0);

function job(state: JobState, extra: Partial<JobView> = {}): JobView {
  return {
    id: 'j',
    tableId: 't',
    state,
    plannedCalls: 9,
    doneCalls: 4,
    pauseReason: null,
    lastError: null,
    nextAttemptAt: null,
    heartbeatAt: null,
    cancelRequested: false,
    createdAt: '2026-10-23T09:00:00Z',
    ...extra,
  };
}

function status(j: JobView | null, extra: Partial<FillStatus> = {}): FillStatus {
  return { job: j, papersDone: 3, papersTotal: 8, callsToday: 12, cap: 30, dayResetsAt: RESET, run: j?.state === 'running' ? 'here' : null, leaseExpiresAt: null, ...extra };
}

const paused = (reason: PauseReason, extra: Partial<JobView> = {}) => job('paused', { pauseReason: reason, ...extra });

test('every pause reason the runner sets has its own words', () => {
  const jobs = read(new URL('jobs.rs', TABLES_DIR));
  const reasons = strings(block(jobs, 'impl PauseReason', 'pub fn parse'), /=> "([a-z_]+)"/g) as PauseReason[];
  assert.equal(reasons.length, 7);
  const fallback = pauseText(paused(null as unknown as PauseReason), NOW, INDIA).text;
  for (const reason of reasons) {
    const text = pauseText(paused(reason, { nextAttemptAt: NOW + 5 * 60 * 1000 }), NOW, INDIA).text;
    assert.notEqual(text, fallback, reason);
  }
});

test('a short rate limit says when it goes on; a daily quota and the cap name the reset', () => {
  const short = pauseText(paused('quota', { lastError: 'model_quota:short', nextAttemptAt: NOW + 2 * 60 * 1000 }), NOW, INDIA);
  assert.equal(short.tone, 'waiting');
  assert.equal(short.text, "Paused for the provider's rate limit; goes on at 15:32.");
  const daily = pauseText(paused('quota', { lastError: 'model_quota:daily', nextAttemptAt: RESET }), NOW, INDIA);
  assert.match(daily.text, /^Paused: the provider's daily quota is used up\. It goes on at 0?5:30 tomorrow\.$/);
  const cap = pauseText(paused('daily_cap', { nextAttemptAt: RESET }), NOW, INDIA);
  assert.match(cap.text, /cap of table calls is reached\. It goes on at 0?5:30 tomorrow/);
});

test('the quota wording follows the window the runner stored, whatever the time left', () => {
  // A daily quota 40 minutes before the UTC reset is still the daily quota, and a short
  // limit far away is still a rate limit: the wording never changes category as time passes.
  const reset = Date.UTC(2026, 9, 24, 0, 0);
  const daily = paused('quota', { lastError: 'model_quota:daily', nextAttemptAt: reset });
  for (const minutesLeft of [600, 120, 40, 1]) {
    assert.match(pauseText(daily, reset - minutesLeft * 60_000, INDIA).text, /daily quota is used up/, `${minutesLeft} min left`);
  }
  const short = paused('quota', { lastError: 'model_quota:short', nextAttemptAt: reset });
  for (const minutesLeft of [600, 40, 1]) {
    assert.match(pauseText(short, reset - minutesLeft * 60_000, INDIA).text, /rate limit; goes on at/, `${minutesLeft} min left`);
  }
  assert.match(pauseText(paused('quota', { lastError: 'model_quota:unknown', nextAttemptAt: reset }), reset - 600 * 60_000, INDIA).text, /rate limit/);
  // A pause stored before the window was kept reads as a rate limit, never as a guess from the wait.
  assert.match(pauseText(paused('quota', { lastError: 'model_quota', nextAttemptAt: reset }), reset - 600 * 60_000, INDIA).text, /rate limit/);
});

test('pauses that wait for the user say so and what to fix', () => {
  const ladder = pauseText(paused('quota', { lastError: 'model_quota' }), NOW, INDIA);
  assert.equal(ladder.tone, 'needs_you');
  assert.equal(pauseText(paused('auth', { lastError: 'model_auth' }), NOW, INDIA).tone, 'needs_you');
  const failing = pauseText(paused('failing', { lastError: 'unparsable_response' }), NOW, INDIA);
  assert.equal(failing.tone, 'needs_you');
  assert.match(failing.text, /could not be read/);
  assert.equal(pauseText(paused('unreachable', { lastError: 'model_timeout' }), NOW, INDIA).tone, 'needs_you');
  assert.match(pauseText(paused('unreachable', { nextAttemptAt: NOW + 60_000 }), NOW, INDIA).text, /trying again at 15:31/);
  assert.match(pauseText(paused('busy', { nextAttemptAt: NOW + 60_000 }), NOW, INDIA).text, /busy.*15:31/);
});

test('a model the fill cannot use is named with its code', () => {
  const words = (code: string) => pauseText(paused('model_unavailable', { lastError: code }), NOW, INDIA);
  assert.equal(words('refused_reasoning').tone, 'needs_you');
  assert.match(words('refused_reasoning').text, /always thinks at length and can't be capped; pick another/);
  assert.match(words('local_only').text, /Local-only mode/);
  assert.match(words('no_key').text, /API key/);
  assert.match(words('no_model').text, /No model is set/);
  for (const code of ['refused_reasoning', 'local_only', 'no_key', 'no_model']) assert.match(words(code).text, /within a minute/);
});

test('the status line shows progress, cancelling, and why a fill stopped', () => {
  assert.equal(fillStatusText(status(null), NOW, INDIA), null);
  const running = fillStatusText(status(job('running')), NOW, INDIA);
  assert.equal(running?.text, 'Filling.');
  assert.equal(running?.progress, '3 of 8 papers done · 4 of about 9 calls.');
  assert.match(fillStatusText(status(job('running', { cancelRequested: true })), NOW, INDIA)?.text ?? '', /^Cancelling…/);
  assert.match(fillStatusText(status(job('failed', { lastError: 'search_failed: the embedder is missing' })), NOW, INDIA)?.text ?? '', /search models may be missing/);
  assert.equal(fillStatusText(status(job('done')), NOW, INDIA)?.tone, 'done');
  assert.equal(fillStatusText(status(paused('daily_cap', { nextAttemptAt: RESET })), NOW, INDIA)?.progress, '3 of 8 papers done · 4 of about 9 calls.');
});

test('the announced text changes with the job state only, never with the counts', () => {
  const a = fillStatusText(status(job('running', { doneCalls: 4 }), { papersDone: 3 }), NOW, INDIA);
  const b = fillStatusText(status(job('running', { doneCalls: 9 }), { papersDone: 6 }), NOW + 60_000, INDIA);
  assert.equal(a?.text, b?.text);
  assert.notEqual(a?.progress, b?.progress);
});

test('a fill another run holds says who and when it goes on here, from the backend', () => {
  const lease = NOW + 4 * 60 * 1000;
  // Heard from a minute ago: another Shodh is filling; no "not responding".
  const fresh = fillStatusText(status(job('running', { heartbeatAt: NOW - 60_000 }), { run: 'elsewhere', leaseExpiresAt: lease }), NOW, INDIA);
  assert.equal(fresh?.tone, 'waiting');
  assert.match(fresh?.text ?? '', /^Another run of Shodh is filling this table \(last heard from at 15:29\)\. If it stops, the fill goes on here by 15:34 at the latest\.$/);
  // Silent past a call's timeout: not responding, with the lease's end.
  const quiet = fillStatusText(status(job('running', { heartbeatAt: NOW - 8 * 60 * 1000 }), { run: 'elsewhere', leaseExpiresAt: NOW + 2 * 60 * 1000 }), NOW, INDIA);
  assert.match(quiet?.text ?? '', /^Not responding: .*not been heard from since 15:22\. The fill goes on here by 15:32 at the latest\.$/);
  // No run holds it: the scheduler takes it up.
  assert.match(fillStatusText(status(job('running'), { run: 'stopped' }), NOW, INDIA)?.text ?? '', /stopped; the fill goes on here within a minute/);
  // Running here, however old the heartbeat looks: filling.
  assert.equal(fillStatusText(status(job('running', { heartbeatAt: NOW - 8 * 60 * 1000 }), { run: 'here' }), NOW, INDIA)?.text, 'Filling.');
});

test('a fill that ended with failed cells says how many, as a warning', () => {
  const done = fillStatusText(status(job('done', { doneCalls: 10 }), { papersDone: 5 }), NOW, INDIA, { failedCells: 4 });
  assert.equal(done?.tone, 'warning');
  assert.equal(done?.text, "Filled 5 papers with 10 calls; 4 cells couldn't be filled (show them with “With failed cells”).");
  assert.equal(fillStatusText(status(job('done')), NOW, INDIA, { failedCells: 0 })?.tone, 'done');
  const since = '2026-10-23T09:00:00.000Z';
  const cells = [
    { status: 'failed' as const, error: 'unparsable_response', filledAt: '2026-10-23T09:05:00.000Z' },
    { status: 'verified' as const, error: 'refill_failed:model_timeout', filledAt: '2026-10-23T09:06:00.000Z' },
    { status: 'failed' as const, error: 'missing_in_reply', filledAt: '2026-10-22T09:00:00.000Z' },
    { status: 'verified' as const, error: null, filledAt: '2026-10-23T09:07:00.000Z' },
  ];
  assert.equal(failedCellsSince(cells, since), 2, 'failures of this job only, kept results whose refresh failed included');
});

test('active jobs and pending cancels', () => {
  for (const state of ['queued', 'running', 'paused'] as const) assert.equal(jobActive(job(state)), true);
  for (const state of ['done', 'cancelled', 'failed'] as const) assert.equal(jobActive(job(state)), false);
  assert.equal(jobActive(null), false);
  assert.equal(cancelPending(job('running', { cancelRequested: true })), true);
  assert.equal(cancelPending(job('running')), false);
  assert.equal(cancelPending(job('cancelled', { cancelRequested: true })), false);
});

// ── Fill, cancel, fill by hand ──────────────────────────────────────

test('each fill_start result is told honestly', () => {
  const start = (s: 'started' | 'already_running' | 'held_elsewhere' | 'at_cap', leaseExpiresAt: number | null = null) =>
    fillStartOutcome({ job: job('running'), start: s, leaseExpiresAt, dayResetsAt: RESET }, NOW, INDIA);
  assert.equal(start('started').tone, 'ok');
  assert.match(start('already_running').text, /being filled already/);
  const held = start('held_elsewhere', NOW + 7 * 60 * 1000);
  assert.equal(held.tone, 'warning');
  assert.match(held.text, /earlier run of Shodh still holds this fill.*15:37 at the latest/);
  assert.match(start('held_elsewhere').text, /once that run lets go/);
  const cap = start('at_cap');
  assert.equal(cap.tone, 'warning');
  assert.match(cap.text, /used up\. The fill goes on at 0?5:30 tomorrow/);
});

test('a cancel asked of a running fill says cancelling until it is done', () => {
  assert.match(cancelOutcome({ job: job('running', { cancelRequested: true }), pending: true, leaseExpiresAt: null }, NOW, INDIA).text, /^Cancelling… the fill stops after its current call/);
  assert.match(cancelOutcome({ job: job('running', { cancelRequested: true }), pending: true, leaseExpiresAt: NOW + 60_000 }, NOW, INDIA).text, /15:31 at the latest/);
  assert.equal(cancelOutcome({ job: job('cancelled'), pending: false, leaseExpiresAt: null }, NOW, INDIA).text, 'Cancelled. Finished cells stay.');
  assert.equal(cancelOutcome({ job: null, pending: false, leaseExpiresAt: null }, NOW, INDIA).text, 'There is no fill to cancel.');
});

function handCell(columnId: string, status: Cell['status'], error: string | null = null): Cell {
  return {
    tableId: 't', rowId: 'r', columnId, status, value: null, quote: null, chunkId: null, page: null, bboxes: null,
    model: null, columnRevision: 1, paperHash: null, filledAt: null, editedAt: null, error,
  };
}

const hand = (extra: Partial<HandFill>): HandFill => ({ cells: [], calls: 1, cacheHits: 0, halted: null, modelError: null, paperError: null, ...extra });
const names: Record<string, string> = { c1: 'Dataset', c2: 'Year', c3: 'Method' };
const nameOf = (id: string) => names[id] ?? id;

test('a fill by hand says what stopped it', () => {
  assert.match(handFillOutcome(hand({ halted: 'daily_cap', calls: 0 })).text, /cap of table calls is reached/);
  const quota = handFillOutcome(hand({ modelError: 'model_quota' }));
  assert.equal(quota.tone, 'error');
  assert.equal(quota.text, reasonText('model_quota'));
  assert.equal(handFillOutcome(hand({ paperError: 'paper_not_indexed' })).text, reasonText('paper_not_indexed'));
  assert.match(handFillOutcome(hand({ calls: 0 })).text, /without a model call/);
  const ok = handFillOutcome(hand({ calls: 2, cells: [handCell('c1', 'verified'), handCell('c2', 'not_found')] }), nameOf);
  assert.equal(ok.tone, 'ok');
  assert.equal(ok.text, 'Answered (2 model calls).');
});

test('a refill whose reply could not be read is an error, never a success', () => {
  // Unparsable twice: no model or paper error, only the failed cell tells it.
  const fill = hand({ calls: 2, cells: [handCell('c1', 'failed', 'unparsable_response')] });
  const outcome = handFillOutcome(fill, nameOf);
  assert.equal(outcome.tone, 'error');
  assert.equal(outcome.text, "The cell couldn't be filled (2 model calls). Dataset: The model's reply could not be read.");
});

test('a refill that kept the earlier result says it could not refresh, as an error', () => {
  const outcome = handFillOutcome(hand({ calls: 1, cells: [handCell('c2', 'verified', 'refill_failed:missing_in_reply')] }), nameOf);
  assert.equal(outcome.tone, 'error');
  assert.match(outcome.text, /^The cell couldn't be filled \(1 model call\)\. Year: Couldn't refresh: The model's reply left this column out/);
});

test('a quality check with some cells failed is a warning naming them; all failed is an error', () => {
  const some = handFillOutcome(
    hand({ calls: 2, cells: [handCell('c1', 'verified'), handCell('c2', 'failed', 'missing_in_reply'), handCell('c3', 'failed', 'unparsable_response')] }),
    nameOf,
  );
  assert.equal(some.tone, 'warning');
  assert.match(some.text, /^2 of 3 cells couldn't be filled \(2 model calls\)\. Year: .*left this column out.* Method: .*could not be read/);
  const all = handFillOutcome(hand({ calls: 3, cells: [handCell('c1', 'failed', 'unparsable_response'), handCell('c2', 'failed', 'unparsable_response')] }), nameOf);
  assert.equal(all.tone, 'error');
  assert.match(all.text, /^None of the 2 cells could be filled \(3 model calls\)\./);
  // A failed cell without a code takes the call's error.
  const timeout = handFillOutcome(hand({ modelError: 'model_timeout', cells: [handCell('c1', 'failed')] }), nameOf);
  assert.match(timeout.text, /Dataset: The model could not be reached\./);
});

test('the estimate and the cap are worded', () => {
  assert.equal(estimateText(8, 5, 9), 'Filling 8 papers × 5 columns needs about 9 model calls.');
  assert.equal(estimateText(1, 1, 1), 'Filling 1 paper × 1 column needs about 1 model call.');
  assert.equal(capNotice(9, 12, 30, RESET, NOW, INDIA), null);
  assert.match(capNotice(9, 21, 30, RESET, NOW, INDIA) ?? '', /^Only 9 calls are left today, so the fill pauses at the cap and goes on at 0?5:30 tomorrow\.$/);
  assert.match(capNotice(1, 29, 30, RESET, NOW, INDIA) ?? '', /used up, so the fill waits until 0?5:30 tomorrow/);
});

// ── A cell ──────────────────────────────────────────────────────────

test('a kept result whose refresh failed shows its value with "couldn\u2019t refresh" and why', () => {
  const kept = cellExplanation({ status: 'verified', error: 'refill_failed:model_timeout' }, false);
  assert.equal(kept.label, 'Verified');
  assert.equal(kept.refreshFailed, "Couldn't refresh: The model could not be reached. The earlier result is kept.");
  const notFound = cellExplanation({ status: 'not_found', error: 'refill_failed:paper_not_indexed' }, false);
  assert.equal(notFound.label, NOT_FOUND_TEXT);
  assert.match(notFound.refreshFailed ?? '', /not indexed yet/);
  assert.equal(notFound.detail, null);
});

test('each cell state is explained, with its reason', () => {
  assert.equal(cellExplanation({ status: 'unverified', error: 'quote_not_in_passages' }, false).detail, reasonText('quote_not_in_passages'));
  assert.match(cellExplanation({ status: 'not_found', error: 'not_found_partial' }, false).detail ?? '', /did not fit/);
  assert.equal(cellExplanation({ status: 'edited', error: 'value_no_longer_fits' }, false).detail, reasonText('value_no_longer_fits'));
  assert.match(cellExplanation({ status: 'failed', error: 'missing_in_reply' }, false).detail ?? '', /left this column out.*tries again/);
  assert.match(cellExplanation({ status: 'verified', error: null }, true).detail ?? '', /earlier version of this column/);
  assert.equal(cellExplanation(undefined, false).label, 'Not filled yet');
  assert.equal(cellExplanation({ status: 'queued', error: null }, false).label, 'Waiting in the fill');
});
