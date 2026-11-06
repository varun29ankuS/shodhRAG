/**
 * Code reviews as the pane shows them (`code_review_commands.rs`,
 * `shodh_rag::code_review`): what the fresh-eyes reviewer was given, its ranked
 * findings, and how a finding is marked once a check the reviewer proposed ran in
 * the Code conversation ("Verified by running" with the command and what it showed,
 * or "From reading only").
 *
 * Also the messages the pane sends to the Code conversation: a check to run and a
 * fix to apply, each then decided there by the conversation's mode like any other
 * command or edit.
 *
 * Pure (type-only imports, no enums) so it is unit-tested directly with Node
 * (`app/tests/codeReview.test.ts`).
 */

import type { DiffLine, FileChange, LineSelection } from './diffModel';

export type ReviewCheck = 'algorithm' | 'security' | 'performance' | 'simpler' | 'architecture' | 'custom';

/** The checks in the order the pane offers them, with their names. */
export const REVIEW_CHECKS: readonly { id: ReviewCheck; label: string; hint: string }[] = [
  { id: 'algorithm', label: 'Algorithm', hint: 'Correctness, edge cases, complexity' },
  { id: 'security', label: 'Security', hint: 'Untrusted input, secrets, races' },
  { id: 'performance', label: 'Performance', hint: 'Repeated work, allocations, blocking' },
  { id: 'simpler', label: 'Simpler version', hint: 'The same behaviour with less code' },
  { id: 'architecture', label: 'Architecture', hint: 'Layers, cycles, boundaries of a folder' },
  { id: 'custom', label: 'Your question', hint: 'Ask the reviewer what to look at' },
];

export type LineKind = 'same' | 'added' | 'removed';

export interface PieceLine {
  number: number;
  kind: LineKind;
  text: string;
}

export interface ReviewPiece {
  path: string;
  language: string;
  lines: PieceLine[];
}

/** Exactly what the reviewer gets (the backend refuses any other field). */
export interface ReviewRequest {
  check: ReviewCheck;
  pieces: ReviewPiece[];
  question: string | null;
  folder: string | null;
  flows: string | null;
  context: string | null;
  canVerify: boolean;
}

export type Severity = 'critical' | 'high' | 'medium' | 'low';

export interface Citation {
  path: string;
  line: number;
  endLine: number;
}

export interface Replacement {
  path: string;
  old: string;
  new: string;
}

export interface VerifyStep {
  command: string;
  expect: 'fails' | 'passes';
  outputContains: string | null;
}

export interface Finding {
  severity: Severity;
  title: string;
  citation: Citation | null;
  scenario: string;
  fix: string;
  replace: Replacement | null;
  edges: [string, string][];
  verify: VerifyStep | null;
}

export interface ReviewReport {
  check: ReviewCheck;
  summary: string;
  findings: Finding[];
  diagram: string | null;
  dropped: number;
}

export interface StoredReview {
  id: string;
  createdAt: string;
  workspaceId: string | null;
  conversationId: string | null;
  byAgent: boolean;
  request: ReviewRequest;
  report: ReviewReport;
}

export const SEVERITY_LABEL: Record<Severity, string> = {
  critical: 'Critical',
  high: 'High',
  medium: 'Medium',
  low: 'Low',
};

/** "src/x.rs:12" or "src/x.rs:12–14". */
export function citationText(citation: Citation): string {
  return citation.line === citation.endLine
    ? `${citation.path}:${citation.line}`
    : `${citation.path}:${citation.line}–${citation.endLine}`;
}

/** The selection a citation jumps to. */
export function citationSelection(citation: Citation): LineSelection {
  return { path: citation.path, from: citation.line, to: Math.max(citation.line, citation.endLine) };
}

/** Lines of a diff or file as a piece for the reviewer: each under its shown number. */
export function pieceFromLines(path: string, language: string, lines: readonly DiffLine[]): ReviewPiece {
  return {
    path,
    language,
    lines: lines.map(line => ({
      number: (line.kind === 'removed' ? line.old : line.new) ?? line.old ?? 1,
      kind: line.kind,
      text: line.text,
    })),
  };
}

/** The pieces of a diff for the reviewer: the selected lines of the selected file, else every file's lines. */
export function piecesFromChanges(
  changes: readonly FileChange[],
  selection: LineSelection | null,
  languageOf: (path: string) => string,
): ReviewPiece[] {
  const chosen = selection ? changes.filter(c => c.path === selection.path) : changes;
  return chosen
    .map(change => {
      const lines = change.hunks.flatMap(h => h.lines).filter(line => {
        if (!selection) return true;
        const n = (line.kind === 'removed' ? line.old : line.new) ?? line.old ?? 0;
        return n >= selection.from && n <= selection.to;
      });
      return pieceFromLines(change.path, languageOf(change.path), lines);
    })
    .filter(piece => piece.lines.length > 0);
}

/** The pieces a reviewer saw, as diffs to show them by (a piece of unchanged lines is a file view). */
export function piecesAsChanges(pieces: readonly ReviewPiece[]): FileChange[] {
  return pieces.map(piece => ({
    path: piece.path,
    status: 'changed' as const,
    binary: false,
    tooLarge: false,
    hunks: [
      {
        lines: piece.lines.map(line => ({
          kind: line.kind,
          text: line.text,
          old: line.kind === 'added' ? null : line.number,
          new: line.kind === 'removed' ? null : line.number,
        })),
      },
    ],
  }));
}

/** The lines a review covered, as a selection to show them by. */
export function reviewedSelection(request: ReviewRequest): LineSelection | null {
  const piece = request.pieces[0];
  if (!piece || piece.lines.length === 0) return null;
  const numbers = piece.lines.map(l => l.number);
  return { path: piece.path, from: Math.min(...numbers), to: Math.max(...numbers) };
}

/** "12 lines of src/x.rs" / "3 files, 80 lines": what the reviewer saw, in words. */
export function reviewedWords(request: ReviewRequest): string {
  if (request.pieces.length === 0) return request.folder ? `the structure of ${request.folder}` : 'nothing';
  const lines = request.pieces.reduce((n, p) => n + p.lines.length, 0);
  const lineWord = `${lines} ${lines === 1 ? 'line' : 'lines'}`;
  if (request.pieces.length === 1) return `${lineWord} of ${request.pieces[0].path}`;
  return `${lineWord} in ${request.pieces.length} files`;
}

/** A check run in the Code conversation, as the pane saw it go. */
export interface CheckRun {
  command: string;
  /** Set once Shodh started it. */
  commandId: string | null;
  state: 'waiting' | 'running' | 'done' | 'timed_out' | 'stopped' | 'killed' | 'refused';
  exitCode: number | null;
  output: string;
  note: string | null;
}

/** Output kept per check, from its end (what shows the result). */
export const MAX_CHECK_OUTPUT = 20_000;

/** `run` with more output, keeping its last [`MAX_CHECK_OUTPUT`] characters. */
export function withOutput(run: CheckRun, text: string): CheckRun {
  const output = (run.output + text).slice(-MAX_CHECK_OUTPUT);
  return { ...run, output };
}

export type Verification =
  | { status: 'reading'; label: string }
  | { status: 'pending'; label: string }
  | { status: 'not_run'; label: string }
  | { status: 'confirmed' | 'not_confirmed'; label: string };

function stateWords(state: CheckRun['state']): string {
  switch (state) {
    case 'timed_out':
      return 'reached its time limit';
    case 'stopped':
      return 'was stopped';
    case 'killed':
      return 'was stopped by a limit';
    case 'refused':
      return 'did not run';
    default:
      return 'did not finish';
  }
}

/**
 * How a finding is marked: from reading only until its check ran to its end; then
 * verified by running, with the command, its exit code, and whether what it showed
 * is what the reviewer said it would if the finding is right.
 */
export function verification(step: VerifyStep | null, run: CheckRun | null): Verification {
  if (!step || !run) return { status: 'reading', label: 'From reading only' };
  if (run.state === 'waiting') return { status: 'pending', label: 'Waiting for the check to run in the conversation' };
  if (run.state === 'running') return { status: 'pending', label: `Running ${run.command}` };
  if (run.state !== 'done') {
    return { status: 'not_run', label: `From reading only: the check ${stateWords(run.state)}${run.note ? ` (${run.note})` : ''}` };
  }
  const exit = run.exitCode ?? 0;
  const failed = exit !== 0;
  const exitMatches = step.expect === 'fails' ? failed : !failed;
  const wanted = step.outputContains?.trim() || null;
  const outputMatches = wanted === null || run.output.includes(wanted);
  const observed = `exit ${exit}${wanted ? (outputMatches ? `, output shows “${wanted}”` : `, output does not show “${wanted}”`) : ''}`;
  return exitMatches && outputMatches
    ? { status: 'confirmed', label: `Verified by running: ran ${run.command}, observed ${observed}. That confirms the finding.` }
    : { status: 'not_confirmed', label: `Verified by running: ran ${run.command}, observed ${observed}. That does not confirm the finding.` };
}

/** A code fence that `text` cannot close: one backtick longer than its longest run. */
export function fenceFor(text: string): string {
  let longest = 0;
  for (const run of text.match(/`+/g) ?? []) longest = Math.max(longest, run.length);
  return '`'.repeat(Math.max(3, longest + 1));
}

function where(finding: Finding): string {
  return finding.citation ? ` at ${citationText(finding.citation)}` : '';
}

/** What the pane sends the Code conversation to run a finding's proposed check. */
export function checkMessage(finding: Finding, step: VerifyStep): string {
  const fence = fenceFor(step.command);
  return [
    `Run this check from the code review with run_command, exactly as written, and change nothing else. The fresh-eyes reviewer proposed it for its finding “${finding.title}”${where(finding)}. When it ends, say its exit code and the output lines that matter.`,
    '',
    `${fence}bash`,
    step.command,
    fence,
  ].join('\n');
}

/** What the pane sends the Code conversation to apply a finding's fix (one edit, asked for like any other). */
export function applyFixMessage(finding: Finding, replace: Replacement): string {
  const oldFence = fenceFor(replace.old);
  const newFence = fenceFor(replace.new);
  return [
    `Apply this fix from the code review as one edit of ${replace.path} with the edit tool, and change nothing else. The reviewer's finding: “${finding.title}”${where(finding)}.`,
    '',
    'Replace exactly:',
    oldFence,
    replace.old,
    oldFence,
    'with:',
    newFence,
    replace.new,
    newFence,
  ].join('\n');
}

/** Why the pane cannot send a check or a fix to a conversation, or null when it can. */
export function conversationProblem(
  target: { mode: 'research' | 'code' | null; agentMode: string | null; workspaceId: string | null; running: boolean } | null,
  reviewWorkspaceId: string | null,
): string | null {
  if (!target || target.mode !== 'code') return 'Checks and fixes go to a Code conversation of this project. Open one to use them.';
  if (reviewWorkspaceId && target.workspaceId !== reviewWorkspaceId) {
    return 'The open conversation works on another project. Open a Code conversation of this one.';
  }
  if (target.agentMode === 'plan') return 'Plan mode only reads: commands and edits do not run. Pick another mode to run checks or apply fixes.';
  if (target.running) return 'An answer is running in the conversation. Wait for it to finish.';
  return null;
}
