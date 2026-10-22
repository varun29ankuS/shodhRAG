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
  fillStartOutcome,
  fillStatusText,
  GENERIC_REASON,
  handFillOutcome,
  jobActive,
  knownReason,
  localTime,
  NOT_FOUND_TEXT,
  pauseText,
  REASON_CODES,
  reasonText,
  splitError,
} from '../src/features/tables/format.ts';
import type { FillStatus, HandFill, JobState, JobView, PauseReason } from '../src/features/tables/types.ts';

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
  return { job: j, papersDone: 3, papersTotal: 8, callsToday: 12, cap: 30, dayResetsAt: RESET, ...extra };
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
  const short = pauseText(paused('quota', { lastError: 'model_quota', nextAttemptAt: NOW + 2 * 60 * 1000 }), NOW, INDIA);
  assert.equal(short.tone, 'waiting');
  assert.equal(short.text, "Paused for the provider's rate limit; goes on at 15:32.");
  const daily = pauseText(paused('quota', { lastError: 'model_quota', nextAttemptAt: RESET }), NOW, INDIA);
  assert.match(daily.text, /^Paused: the provider's daily quota is used up\. It goes on at 0?5:30 tomorrow\.$/);
  const cap = pauseText(paused('daily_cap', { nextAttemptAt: RESET }), NOW, INDIA);
  assert.match(cap.text, /cap of table calls is reached\. It goes on at 0?5:30 tomorrow/);
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
  assert.equal(fillStatusText(status(job('running')), NOW, INDIA)?.text, 'Filling: 3 of 8 papers done · 4 of about 9 calls.');
  assert.match(fillStatusText(status(job('running', { cancelRequested: true })), NOW, INDIA)?.text ?? '', /^Cancelling…/);
  assert.match(fillStatusText(status(job('failed', { lastError: 'search_failed: the embedder is missing' })), NOW, INDIA)?.text ?? '', /search models may be missing/);
  assert.equal(fillStatusText(status(job('done')), NOW, INDIA)?.tone, 'done');
  assert.match(fillStatusText(status(paused('daily_cap', { nextAttemptAt: RESET })), NOW, INDIA)?.text ?? '', /3 of 8 papers done/);
});

test('a fill held by a run that stopped answering says when it goes on', () => {
  const quiet = job('running', { heartbeatAt: NOW - 8 * 60 * 1000 });
  const line = fillStatusText(status(quiet), NOW, INDIA);
  assert.equal(line?.tone, 'waiting');
  assert.match(line?.text ?? '', /not been heard from since 15:22\..*by 15:32 at the latest/);
  assert.match(fillStatusText(status(job('running', { heartbeatAt: NOW - 60_000 })), NOW, INDIA)?.text ?? '', /^Filling:/);
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

test('a fill by hand says what stopped it', () => {
  const hand = (extra: Partial<HandFill>): HandFill => ({ cells: [], calls: 1, cacheHits: 0, halted: null, modelError: null, paperError: null, ...extra });
  assert.match(handFillOutcome(hand({ halted: 'daily_cap', calls: 0 })).text, /cap of table calls is reached/);
  assert.equal(handFillOutcome(hand({ modelError: 'model_quota' })).text, reasonText('model_quota'));
  assert.equal(handFillOutcome(hand({ paperError: 'paper_not_indexed' })).text, reasonText('paper_not_indexed'));
  assert.match(handFillOutcome(hand({ calls: 0 })).text, /without a model call/);
  assert.equal(handFillOutcome(hand({ calls: 2 })).text, 'Asked the model (2 calls).');
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
