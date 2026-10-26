/**
 * One Code-mode command run by `run_command` in its own process, as the
 * transcript keeps it: the command, how it ended, and the tail of its output
 * (stdout and stderr interleaved as they arrived). Pure, no React, type-only
 * imports besides `format.ts`, so it is unit-tested directly with Node
 * (`app/tests/commandRun.test.ts`).
 */

import type { CommandState } from './events';
import { formatMs } from './format.ts';

/** Where a command is: still running, or how it ended. */
export type CommandRunState = 'running' | CommandState;

export interface CommandRun {
  /** Id of the command's process run (`run_command_stop` takes it). */
  commandId: string;
  command: string;
  state: CommandRunState;
  /** Clock time it started, ms since epoch (the step's start when it never started). */
  startedAtMs: number;
  /** Wall-clock limit in seconds; null when unknown (it never started). */
  timeLimitSecs: number | null;
  exitCode: number | null;
  /** What acted on it, in words (a limit, why it was refused, stopped background processes). */
  note: string | null;
  /** Wall time once it ended. */
  durationMs: number | null;
  /** The latest output, at most `MAX_COMMAND_OUTPUT_CHARS`. */
  output: string;
  /** Earlier output was dropped to keep the tail. */
  outputCut: boolean;
}

/** Most output characters kept for a command while the answer runs (the tail). */
export const MAX_COMMAND_OUTPUT_CHARS = 64 * 1024;

/** Most output characters stored with the conversation per command (the tail). */
export const MAX_PERSISTED_COMMAND_OUTPUT_CHARS = 16 * 1024;

/** Note of a command still running when its answer ended. */
export const ENDED_WITH_ANSWER_NOTE = 'Ended with the answer';

/** Note of a command still running when the app closed. */
export const APP_CLOSED_NOTE = 'The app closed before it finished';

const STATES: readonly CommandRunState[] = ['running', 'done', 'timed_out', 'stopped', 'killed', 'refused'];

/** The last `max` characters of `text`, never starting inside a surrogate pair. */
function tail(text: string, max: number): string {
  if (text.length <= max) return text;
  let start = text.length - Math.max(0, max);
  const code = text.charCodeAt(start);
  if (code >= 0xdc00 && code <= 0xdfff) start += 1;
  return text.slice(start);
}

/** A command that has just started. */
export function startCommandRun(start: {
  commandId: string;
  command: string;
  atMs: number;
  timeLimitSecs: number | null;
}): CommandRun {
  return {
    commandId: start.commandId,
    command: start.command,
    state: 'running',
    startedAtMs: start.atMs,
    timeLimitSecs: start.timeLimitSecs,
    exitCode: null,
    note: null,
    durationMs: null,
    output: '',
    outputCut: false,
  };
}

/** `run` with `text` appended, keeping only the last `max` characters (and saying so). */
export function appendCommandOutput(run: CommandRun, text: string, max: number = MAX_COMMAND_OUTPUT_CHARS): CommandRun {
  if (text.length === 0) return run;
  const combined = run.output + text;
  const output = tail(combined, max);
  return { ...run, output, outputCut: run.outputCut || output.length < combined.length };
}

/** `run` with its output cut to the last `max` characters (unchanged when it fits). */
export function trimCommandOutput(run: CommandRun, max: number): CommandRun {
  if (run.output.length <= max) return run;
  return { ...run, output: tail(run.output, max), outputCut: true };
}

/** `run` as the backend reported its end. */
export function finishCommandRun(
  run: CommandRun,
  end: { state: CommandState; exitCode: number | null; note: string | null; durationMs: number },
): CommandRun {
  return {
    ...run,
    state: end.state,
    exitCode: end.exitCode,
    note: end.note,
    durationMs: Math.max(0, end.durationMs),
  };
}

/**
 * A running command whose end will never be reported (the answer ended, or
 * the app closed) as stopped at `atMs` (null when that time is unknown), with
 * `note` saying why. Any other run is returned unchanged.
 */
export function stopUnfinishedCommand(run: CommandRun, note: string, atMs: number | null): CommandRun {
  if (run.state !== 'running') return run;
  return { ...run, state: 'stopped', note, durationMs: atMs === null ? null : Math.max(0, atMs - run.startedAtMs) };
}

/** How long a running command has been running at `nowMs` (never negative). */
export function commandElapsedMs(run: CommandRun, nowMs: number): number {
  return Math.max(0, nowMs - run.startedAtMs);
}

/** Whole-second running time: "0s", "12s", "1m 05s", "1h 00m 00s". */
export function formatElapsed(ms: number): string {
  const total = Number.isFinite(ms) && ms > 0 ? Math.floor(ms / 1000) : 0;
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  if (hours > 0) return `${hours}h ${String(minutes).padStart(2, '0')}m ${String(seconds).padStart(2, '0')}s`;
  if (minutes > 0) return `${minutes}m ${String(seconds).padStart(2, '0')}s`;
  return `${seconds}s`;
}

/** A time limit in words: "45s", "10 min", "1 h", "1h 30m", "2m 30s". */
export function formatTimeLimit(secs: number): string {
  if (!Number.isFinite(secs) || secs <= 0) return '0s';
  const whole = Math.round(secs);
  if (whole < 60) return `${whole}s`;
  if (whole % 3600 === 0) return `${whole / 3600} h`;
  if (whole % 60 === 0) {
    const minutes = whole / 60;
    return minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
  }
  return formatElapsed(whole * 1000);
}

/** The command's state in words: "Running", "Done (exit 0)", "Killed: memory limit (4096 MB)", … */
export function commandStateLabel(run: CommandRun): string {
  switch (run.state) {
    case 'running':
      return 'Running';
    case 'done':
      return run.exitCode === null ? 'Done' : `Done (exit ${run.exitCode})`;
    case 'timed_out':
      return 'Timed out';
    case 'stopped':
      return 'Stopped';
    case 'killed':
      return run.note ? `Killed: ${run.note}` : 'Killed: a limit was reached';
    case 'refused':
      return run.note ? `Refused: ${run.note}` : 'Refused';
    default:
      return 'Stopped';
  }
}

/** Whether the command ended in a way the user should notice (not a clean exit or a stop). */
export function commandFailed(run: CommandRun): boolean {
  if (run.state === 'done') return run.exitCode !== null && run.exitCode !== 0;
  return run.state === 'timed_out' || run.state === 'killed' || run.state === 'refused';
}

/** One line for a finished command: "Done (exit 0) · 3.2s" (no duration for a refused one). */
export function commandSummary(run: CommandRun): string {
  const label = commandStateLabel(run);
  if (run.state === 'refused' || run.durationMs === null) return label;
  return `${label} · ${formatMs(run.durationMs)}`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function finiteOrNull(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function textOrNull(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 ? value : null;
}

/** A stored command run, validated and capped; null when it is not one. */
export function readCommandRun(value: unknown): CommandRun | null {
  if (!isRecord(value)) return null;
  if (typeof value.commandId !== 'string' || typeof value.command !== 'string') return null;
  if (!STATES.includes(value.state as CommandRunState)) return null;
  if (typeof value.startedAtMs !== 'number' || !Number.isFinite(value.startedAtMs)) return null;
  const output = typeof value.output === 'string' ? value.output : '';
  const run: CommandRun = {
    commandId: value.commandId,
    command: value.command,
    state: value.state as CommandRunState,
    startedAtMs: value.startedAtMs,
    timeLimitSecs: finiteOrNull(value.timeLimitSecs),
    exitCode: finiteOrNull(value.exitCode),
    note: textOrNull(value.note),
    durationMs: finiteOrNull(value.durationMs),
    output,
    outputCut: value.outputCut === true,
  };
  return trimCommandOutput(run, MAX_COMMAND_OUTPUT_CHARS);
}
