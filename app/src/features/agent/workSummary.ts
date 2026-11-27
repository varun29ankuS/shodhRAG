import { currentStep, notAnswerBlocks } from './reducer.ts';
import type { TranscriptState, TranscriptStep } from './reducer';

/**
 * How an answer folds its working into one summary line ("Searched 3 sources · read 2 files
 * · 14 s") that opens on click: blocks before `answerIndex` (tool steps, steering, interim
 * notes like "I'll read the paper first", revisions) fold; the answer text stays open.
 * While the answer runs, the line says what is happening now (the current step).
 */
export interface WorkFold {
  /** Index of the answer's first open block; blocks before it fold. */
  answerIndex: number;
  label: string;
  /** The run is going on: the label leads with what is happening now. */
  live: boolean;
}

/** A run's wall time in words: "14 s", "2 min 05 s", then "1 h 01 min" and "2 d 3 h". */
export function seconds(ms: number): string {
  const s = Math.max(1, Math.round(ms / 1000));
  if (s < 60) return `${s} s`;
  const minutes = Math.floor(s / 60);
  if (minutes < 60) return `${minutes} min ${String(s % 60).padStart(2, '0')} s`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ${String(minutes % 60).padStart(2, '0')} min`;
  return `${Math.floor(hours / 24)} d ${hours % 24} h`;
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** What kind of work a step was, for the summary's counts. */
export type StepKind = 'search' | 'read' | 'edit' | 'command' | 'other';

export function stepKind(tool: string | undefined): StepKind {
  if (!tool) return 'other';
  if (tool === 'run_command') return 'command';
  if (/search/.test(tool)) return 'search';
  if (/^(read|fetch|open|show|list|grep|glob|view)(_|$)/.test(tool)) return 'read';
  if (/^(write|edit|apply|create|patch|delete|move|rename)(_|$)/.test(tool)) return 'edit';
  return 'other';
}

/** The counts part of the line: "searched 3 sources", "read 2 files", "1 step". */
function counts(transcript: TranscriptState, folded: TranscriptState['blocks']): string[] {
  const kinds: Record<StepKind, number> = { search: 0, read: 0, edit: 0, command: 0, other: 0 };
  for (const block of folded) {
    if (block.kind !== 'step') continue;
    const step: TranscriptStep | undefined = transcript.steps[block.stepId];
    kinds[stepKind(step?.tool)] += 1;
  }
  const sources = new Set(transcript.passages.map(p => p.path)).size;
  const parts: string[] = [];
  if (sources > 0) parts.push(`searched ${plural(sources, 'source', 'sources')}`);
  else if (kinds.search > 0) parts.push(plural(kinds.search, 'search', 'searches'));
  if (kinds.read > 0) parts.push(`read ${plural(kinds.read, 'file', 'files')}`);
  if (kinds.edit > 0) parts.push(`edited ${plural(kinds.edit, 'file', 'files')}`);
  if (kinds.command > 0) parts.push(`ran ${plural(kinds.command, 'command', 'commands')}`);
  if (kinds.other > 0) parts.push(plural(kinds.other, 'step', 'steps'));
  // A retrieve revision searched the documents (its search is one of the steps); it re-checked
  // nothing.
  const rechecked = folded.reduce(
    (sum, b) => sum + (b.kind === 'revision' && b.reason !== 'retrieve' ? b.flagged : 0),
    0,
  );
  if (rechecked > 0) parts.push(`re-checked ${plural(rechecked, 'statement', 'statements')}`);
  return parts;
}

function sentence(parts: readonly string[]): string {
  const line = parts.join(' · ');
  return line.charAt(0).toUpperCase() + line.slice(1);
}

/**
 * Null when nothing folds: failed, or no work (no tool step, no revision) before the answer.
 * A finished answer folds everything before its final text; a running one folds every block
 * up to its last piece of work, and the line leads with what is happening now.
 */
export function workFold(transcript: TranscriptState): WorkFold | null {
  const blocks = transcript.blocks;
  const live = transcript.status === 'running' || transcript.status === 'starting';
  if (!live && transcript.status !== 'completed') return null;

  let answerIndex = -1;
  if (live) {
    // Everything up to the last step, steer or revision folds; text after it is the answer
    // being written.
    for (let i = blocks.length - 1; i >= 0; i--) {
      if (blocks[i].kind !== 'text') {
        answerIndex = i + 1;
        break;
      }
    }
  } else {
    // The answer is the last text block that a revision did not replace and that is not a
    // working note before tool calls.
    const excluded = notAnswerBlocks(transcript);
    for (let i = blocks.length - 1; i >= 0; i--) {
      const block = blocks[i];
      if (block.kind === 'text' && !excluded.has(block.id)) {
        answerIndex = i;
        break;
      }
    }
  }
  if (answerIndex <= 0) return null;
  const folded = blocks.slice(0, answerIndex);
  if (!folded.some(b => b.kind === 'step' || b.kind === 'revision')) return null;

  const parts = counts(transcript, folded);
  if (live) {
    const step = currentStep(transcript);
    const now = step
      ? step.status === 'awaiting_approval' ? `Waiting for you: ${step.label}` : step.label
      : answerIndex < blocks.length ? 'Writing the answer' : 'Working';
    return { answerIndex, label: [now, ...parts].join(' · '), live: true };
  }
  if (transcript.durationMs !== null) parts.push(seconds(transcript.durationMs));
  return { answerIndex, label: parts.length > 0 ? sentence(parts) : 'Worked', live: false };
}
