/**
 * Words of the Tables tab: every reason, fill and command code in plain words, the fill's
 * status and pause reasons, what a Fill or Cancel did, and the day's calls with the reset
 * in the user's local time. Pure (type-only imports), so `node --test` loads it directly
 * (`app/tests/tablesFormat.test.ts`, which also checks this list against the Rust codes).
 */

import type { Cell, FillCancel, FillStart, FillStatus, HandFill, JobView, PauseReason } from './types.ts';

/** How an empty answer is worded, always: the model saw only some passages. */
export const NOT_FOUND_TEXT = 'Not found in the passages searched';

/** The words for a code nobody listed here. */
export const GENERIC_REASON = 'Something went wrong that Shodh cannot describe yet.';

const SETTINGS_MODEL = 'Settings → Model';

/**
 * Every code the tables backend sends (`research::tables` validate, fill, store and job
 * codes; the model refusals of `memory_learn::TableModelError`; the command error codes),
 * in plain words. The single source: `REASON_CODES` is its keys.
 */
const REASONS: Readonly<Record<string, string>> = {
  // validate.rs: why an answer is unverified or failed (mirrors `validate::reason`).
  quote_not_in_passages: 'The quote does not occur in the passages the model was given.',
  quote_does_not_support: 'The quote appears to contradict the answer.',
  missing_quote: 'The model gave no quote for its answer.',
  missing_answer: "The model's reply had no answer for this column.",
  quote_too_short: 'The quote is too short to back the answer.',
  not_a_number: 'The answer is not a single number.',
  number_not_in_quote: 'The number does not occur in the quote.',
  unit_not_in_quote: 'The unit does not occur in the quote.',
  not_yes_no: 'The answer is not yes or no.',
  not_an_option: "The answer is not one of the column's options.",
  empty_text: 'The answer is empty.',
  text_too_long: 'The answer is longer than 500 characters.',
  not_found_partial: 'Not found, but some passages did not fit in the call, so the answer may be in them. The next fill looks once more.',
  not_found_partial_retried: 'Not found, but some passages did not fit in the call even on the second try, so the answer may be in them.',
  value_no_longer_fits: "Your value does not fit the column's type or options any more.",
  // fill.rs and store.rs: why a paper or a cell failed.
  paper_unreadable: "The paper's file could not be read.",
  paper_not_indexed: 'The paper is not indexed yet, so there is nothing to search in it. Index it in the Library, then fill again.',
  search_failed: 'The search could not look into the paper. The search models may be missing (Settings → Search).',
  model_failed: 'The model refused the request or answered with an error.',
  model_timeout: 'The model could not be reached.',
  model_auth: `The provider refused the API key (${SETTINGS_MODEL}).`,
  model_quota: "The provider's rate limit or quota stopped the call.",
  unparsable_response: "The model's reply could not be read.",
  missing_in_reply: "The model's reply left this column out; the next fill asks again.",
  refill_failed: 'Asking again failed; the earlier result is kept.',
  // jobs.rs and the fill's halts.
  fill_error: 'The fill stopped on an error of its own (the database or a task).',
  fill_panicked: 'The fill crashed twice, so it was stopped.',
  database_busy: 'The database stayed busy with another write.',
  cancelled: 'The fill was cancelled.',
  daily_cap: "Today's cap of table calls is reached.",
  // memory_learn.rs: why no model can fill tables (`TableModelError::code`).
  refused_reasoning: `This model always thinks at length and can't be capped; pick another model for tables (${SETTINGS_MODEL}).`,
  local_only: `Local-only mode is on, so tables need a local model (${SETTINGS_MODEL}).`,
  no_key: `The model's provider needs an API key (${SETTINGS_MODEL}).`,
  no_model: `No model is set (${SETTINGS_MODEL}).`,
  settings_unreadable: 'The settings could not be read.',
  // Command errors (`ResearchCommandError` and the tables' own).
  not_found: 'It no longer exists; it may have been deleted.',
  invalid: 'That was not accepted.',
  unavailable: 'Evidence tables are not available right now.',
  storage: 'The table could not be read or saved.',
  busy: 'The database is busy with another write; try again in a moment.',
  model_refused: `The model set for tables always thinks at length and can't be capped; pick another (${SETTINGS_MODEL}).`,
  model_unavailable: `No model can fill tables now: set a model and its API key in ${SETTINGS_MODEL}, or use a local model in Local-only mode.`,
  fill_running: 'This table is being filled; wait for the fill to end or cancel it first.',
  hand_fill_running: 'This paper is being filled already.',
  diagnostics_off: 'Quality checks are a diagnostics tool: turn them on in Settings → Tables.',
};

/** Every code with its own words. */
export const REASON_CODES: readonly string[] = Object.keys(REASONS);

/** A stored error read apart: `refill_failed:<code>`, `<code>: <detail>`, or a bare code. */
export interface ErrorParts {
  code: string;
  detail: string | null;
  /** A refill failed and the earlier result was kept (`refill_failed:<code>`). */
  refillFailed: boolean;
}

export function splitError(error: string): ErrorParts {
  const text = error.trim();
  const refill = /^refill_failed\s*:(.*)$/s.exec(text);
  if (refill) {
    const inner = splitError(refill[1]);
    return { code: inner.code || 'refill_failed', detail: inner.detail, refillFailed: true };
  }
  const colon = text.indexOf(':');
  if (colon < 0) return { code: text, detail: null, refillFailed: false };
  const detail = text.slice(colon + 1).trim();
  return { code: text.slice(0, colon).trim(), detail: detail || null, refillFailed: false };
}

/** A code (or a stored error holding one) in plain words; never empty. */
export function reasonText(code: string | null | undefined): string {
  if (typeof code !== 'string' || !code.trim()) return GENERIC_REASON;
  const parts = splitError(code);
  const words = Object.prototype.hasOwnProperty.call(REASONS, parts.code) ? REASONS[parts.code] : GENERIC_REASON;
  return parts.refillFailed ? `Couldn't refresh: ${words}` : words;
}

/** Whether a code has words of its own (not the generic text). */
export function knownReason(code: string): boolean {
  return Object.prototype.hasOwnProperty.call(REASONS, splitError(code).code);
}

// ── Time ────────────────────────────────────────────────────────────

/** Where and how to show times: the user's locale and time zone unless a test fixes them. */
export interface TimeOptions {
  locale?: string;
  timeZone?: string;
}

function dateKey(ms: number, options: TimeOptions): string {
  return new Intl.DateTimeFormat('en-CA', { year: 'numeric', month: '2-digit', day: '2-digit', timeZone: options.timeZone }).format(new Date(ms));
}

/**
 * A UTC instant as the user's local time, relative to `now`: "05:30" today, "05:30
 * tomorrow", else with the date ("05:30 on Mon 3 Nov").
 */
export function localTime(ms: number, now: number, options: TimeOptions = {}): string {
  const at = new Date(ms);
  const time = new Intl.DateTimeFormat(options.locale, { hour: 'numeric', minute: '2-digit', timeZone: options.timeZone }).format(at);
  const day = dateKey(ms, options);
  if (day === dateKey(now, options)) return time;
  if (day === dateKey(now + 24 * 60 * 60 * 1000, options)) return `${time} tomorrow`;
  const date = new Intl.DateTimeFormat(options.locale, { weekday: 'short', day: 'numeric', month: 'short', timeZone: options.timeZone }).format(at);
  return `${time} on ${date}`;
}

/** "Tables have made 12 of 30 calls today; resets at 05:30." */
export function callsLine(callsToday: number, cap: number, dayResetsAt: number, now: number, options: TimeOptions = {}): string {
  return `Tables have made ${callsToday} of ${cap} ${cap === 1 ? 'call' : 'calls'} today; resets at ${localTime(dayResetsAt, now, options)}.`;
}

// ── Fill status ─────────────────────────────────────────────────────

/** A short wait (a rate limit) against a long one (the provider's daily quota). */
export const SHORT_WAIT_MS = 60 * 60 * 1000;

export type StatusTone = 'active' | 'waiting' | 'needs_you' | 'done' | 'failed' | 'cancelled';

export interface StatusText {
  tone: StatusTone;
  text: string;
}

function lastCode(job: JobView): string | null {
  return job.lastError ? splitError(job.lastError).code : null;
}

/** Why a job is paused, with when it goes on or what the user should do. */
export function pauseText(job: JobView, now: number, options: TimeOptions = {}): StatusText {
  const reason: PauseReason | null = job.pauseReason;
  const until = job.nextAttemptAt;
  const at = until !== null ? localTime(until, now, options) : null;
  switch (reason) {
    case 'daily_cap':
      return { tone: 'waiting', text: `Paused: today's cap of table calls is reached. It goes on at ${at ?? 'the next reset'}; raise the cap in Settings → Tables to go on now.` };
    case 'quota':
      if (at === null) {
        return { tone: 'needs_you', text: "Paused: the provider's rate limit kept refusing calls. Start the fill again later, or pick another model." };
      }
      if (until - now <= SHORT_WAIT_MS) {
        return { tone: 'waiting', text: `Paused for the provider's rate limit; goes on at ${at}.` };
      }
      return { tone: 'waiting', text: `Paused: the provider's daily quota is used up. It goes on at ${at}.` };
    case 'auth':
      return { tone: 'needs_you', text: `Needs you: the provider refused the API key. Fix it in ${SETTINGS_MODEL}, then start the fill again.` };
    case 'unreachable':
      return at === null
        ? { tone: 'needs_you', text: `Needs you: the model could not be reached for a while. Check the connection or the model in ${SETTINGS_MODEL}, then start the fill again.` }
        : { tone: 'waiting', text: `Paused: the model could not be reached; trying again at ${at}.` };
    case 'failing': {
      const code = lastCode(job);
      return { tone: 'needs_you', text: `Needs you: several papers in a row failed (${reasonText(code)}) Check the model in ${SETTINGS_MODEL}, then start the fill again.` };
    }
    case 'busy':
      return { tone: 'waiting', text: `Paused: the database was busy with another write; goes on at ${at ?? 'the next check'}.` };
    case 'model_unavailable': {
      const code = lastCode(job);
      return { tone: 'needs_you', text: `Needs you: ${reasonText(code ?? 'model_unavailable')} The fill goes on by itself within a minute once that is fixed.` };
    }
    default:
      return at === null
        ? { tone: 'needs_you', text: 'Paused until you start the fill again.' }
        : { tone: 'waiting', text: `Paused; goes on at ${at}.` };
  }
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/**
 * How long a run may be silent: its heartbeat is renewed before every paper and every
 * model call, and one call times out after five minutes (`LEASE_MINUTES` explains it).
 */
export const QUIET_RUN_MS = 6 * 60 * 1000;
/** How long another run's hold on a fill lasts after its last heartbeat (`LEASE_MINUTES`). */
export const LEASE_MS = 10 * 60 * 1000;

/** The table's fill in one line, or null when it never had one. */
export function fillStatusText(status: FillStatus, now: number, options: TimeOptions = {}): StatusText | null {
  const job = status.job;
  if (!job) return null;
  const progress = `${status.papersDone} of ${plural(status.papersTotal, 'paper', 'papers')} done · ${job.doneCalls} of about ${plural(job.plannedCalls, 'call', 'calls')}`;
  switch (job.state) {
    case 'running':
      if (job.heartbeatAt !== null && now - job.heartbeatAt > QUIET_RUN_MS) {
        // Held by a run that stopped answering (an earlier Shodh that quit without letting go).
        return {
          tone: 'waiting',
          text: `${job.cancelRequested ? 'Cancelling… ' : ''}The run filling this table has not been heard from since ${localTime(job.heartbeatAt, now, options)}. If it was an earlier Shodh, the fill goes on here by itself by ${localTime(job.heartbeatAt + LEASE_MS, now, options)} at the latest. ${progress}.`,
        };
      }
      return job.cancelRequested
        ? { tone: 'active', text: `Cancelling… the fill stops after its current call. ${progress}.` }
        : { tone: 'active', text: `Filling: ${progress}.` };
    case 'queued':
      return { tone: 'active', text: job.cancelRequested ? 'Cancelling…' : `Waiting to start. ${progress}.` };
    case 'paused': {
      const pause = pauseText(job, now, options);
      return { tone: pause.tone, text: `${pause.text} ${progress}.` };
    }
    case 'done':
      return { tone: 'done', text: `Filled ${plural(status.papersDone, 'paper', 'papers')} with ${plural(job.doneCalls, 'call', 'calls')}.` };
    case 'cancelled':
      return { tone: 'cancelled', text: `Cancelled after ${plural(job.doneCalls, 'call', 'calls')}; finished cells stay.` };
    case 'failed':
      return { tone: 'failed', text: `The fill stopped: ${reasonText(job.lastError)}` };
    default:
      return null;
  }
}

/** Whether the fill can still go on (polling and the Cancel button follow this). */
export function jobActive(job: JobView | null): boolean {
  return job !== null && (job.state === 'queued' || job.state === 'running' || job.state === 'paused');
}

/** A cancel asked and not done yet. */
export function cancelPending(job: JobView | null): boolean {
  return job !== null && job.cancelRequested && (job.state === 'running' || job.state === 'queued');
}

// ── What Fill, Cancel and a fill by hand did ────────────────────────

export interface Outcome {
  tone: 'ok' | 'info' | 'warning';
  text: string;
}

/** What `fill_start` did, honestly: a fill another run holds is not reported as running. */
export function fillStartOutcome(start: FillStart, now: number, options: TimeOptions = {}): Outcome {
  switch (start.start) {
    case 'started':
      return { tone: 'ok', text: 'Filling started. Cells fill in as each paper is done.' };
    case 'already_running':
      return { tone: 'info', text: 'This table is being filled already.' };
    case 'held_elsewhere':
      return {
        tone: 'warning',
        text: start.leaseExpiresAt !== null
          ? `An earlier run of Shodh still holds this fill. It goes on by itself by ${localTime(start.leaseExpiresAt, now, options)} at the latest.`
          : 'An earlier run of Shodh still holds this fill. It goes on by itself once that run lets go.',
      };
    case 'at_cap':
      return { tone: 'warning', text: `Today's table calls are used up. The fill goes on at ${localTime(start.dayResetsAt, now, options)}, or raise the cap in Settings → Tables.` };
    default:
      return { tone: 'info', text: 'The fill was asked for.' };
  }
}

/** What `fill_cancel` did. */
export function cancelOutcome(cancel: FillCancel, now: number, options: TimeOptions = {}): Outcome {
  if (!cancel.job) return { tone: 'info', text: 'There is no fill to cancel.' };
  if (cancel.pending) {
    return {
      tone: 'info',
      text: cancel.leaseExpiresAt !== null
        ? `Cancelling… an earlier run of Shodh holds the fill; it stops by ${localTime(cancel.leaseExpiresAt, now, options)} at the latest.`
        : 'Cancelling… the fill stops after its current call.',
    };
  }
  return { tone: 'ok', text: 'Cancelled. Finished cells stay.' };
}

/** What a fill by hand (Refill, a quality check) did. */
export function handFillOutcome(fill: HandFill): Outcome {
  if (fill.halted === 'daily_cap') return { tone: 'warning', text: `${reasonText('daily_cap')} Nothing was asked; raise the cap in Settings → Tables or wait for the reset.` };
  if (fill.halted) return { tone: 'info', text: reasonText(fill.halted) };
  if (fill.modelError) return { tone: 'warning', text: reasonText(fill.modelError) };
  if (fill.paperError) return { tone: 'warning', text: reasonText(fill.paperError) };
  if (fill.calls === 0) return { tone: 'ok', text: 'Answered without a model call (the cell was current or cached).' };
  return { tone: 'ok', text: `Asked the model (${plural(fill.calls, 'call', 'calls')}).` };
}

/** The codes a fill by hand is refused with that are a wait, not a fault: shown inline. */
export const INLINE_REFUSALS: readonly string[] = ['fill_running', 'hand_fill_running', 'diagnostics_off'];

// ── Estimate ────────────────────────────────────────────────────────

/** "Filling 8 papers × 5 columns needs about 9 model calls." */
export function estimateText(papers: number, columns: number, calls: number): string {
  return `Filling ${plural(papers, 'paper', 'papers')} × ${plural(columns, 'column', 'columns')} needs about ${plural(calls, 'model call', 'model calls')}.`;
}

/**
 * What the day's cap means for a fill of `calls` calls, or null when it fits. A paper is
 * started only with room for its calls and one retry, so a fill near the cap pauses early.
 */
export function capNotice(
  calls: number,
  callsToday: number,
  cap: number,
  dayResetsAt: number,
  now: number,
  options: TimeOptions = {},
): string | null {
  const left = Math.max(0, cap - callsToday);
  const reset = localTime(dayResetsAt, now, options);
  if (left < 2) return `Today's table calls are used up, so the fill waits until ${reset}. Raise the cap in Settings → Tables to fill now.`;
  if (calls + 1 > left) {
    return `Only ${plural(left, 'call is', 'calls are')} left today, so the fill pauses at the cap and goes on at ${reset}.`;
  }
  return null;
}

// ── A cell ──────────────────────────────────────────────────────────

/** What a cell holds and why, for its panel. */
export interface CellExplanation {
  /** The cell's state in a few words. */
  label: string;
  /** Why, when there is something to say. */
  detail: string | null;
  /** A refill failed and this is the earlier result, kept: the reason it could not be refreshed. */
  refreshFailed: string | null;
}

/**
 * A cell's state and its reason. `stale`: answered for an earlier version of its column.
 * A kept result whose refresh failed (`refill_failed:<code>`) shows its value with
 * "couldn't refresh" and the reason.
 */
export function cellExplanation(cell: Pick<Cell, 'status' | 'error'> | undefined, stale: boolean): CellExplanation {
  const status = cell?.status ?? 'empty';
  const parts = cell?.error ? splitError(cell.error) : null;
  const refreshFailed = parts && parts.refillFailed ? `Couldn't refresh: ${reasonText(parts.code)} The earlier result is kept.` : null;
  const staleText = stale ? ' It was answered for an earlier version of this column; the next fill asks again.' : '';
  switch (status) {
    case 'verified':
      return { label: 'Verified', detail: `The quote occurs in the paper and the value passed its type check.${staleText}`, refreshFailed };
    case 'unverified':
      return { label: 'Unverified', detail: `${parts && !parts.refillFailed ? reasonText(parts.code) : 'The answer could not be checked against the paper.'}${staleText}`, refreshFailed };
    case 'not_found':
      return {
        label: NOT_FOUND_TEXT,
        detail: parts && !parts.refillFailed ? `${reasonText(parts.code)}${staleText}` : staleText.trim() || null,
        refreshFailed,
      };
    case 'edited':
      return {
        label: 'Edited by you',
        detail: parts && !parts.refillFailed ? reasonText(parts.code) : 'A fill never overwrites your value; clear it to have the model answer again.',
        refreshFailed: null,
      };
    case 'failed':
      return { label: 'Failed', detail: `${reasonText(cell?.error)} The next fill tries again.`, refreshFailed: null };
    case 'queued':
      return { label: 'Waiting in the fill', detail: null, refreshFailed: null };
    default:
      return { label: 'Not filled yet', detail: 'Fill the table to answer it.', refreshFailed: null };
  }
}
