import { notAnswerBlocks } from './reducer.ts';
import type { TranscriptState } from './reducer';

/**
 * How a finished answer folds its working: blocks before `answerIndex`
 * (tool steps, steering, interim notes like "I'll read the paper first")
 * collapse into one summary line; the final answer text stays open.
 */
export interface WorkFold {
  /** Index of the final text block; blocks before it fold. */
  answerIndex: number;
  label: string;
}

function seconds(ms: number): string {
  const s = Math.max(1, Math.round(ms / 1000));
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, '0')}s`;
}

/**
 * Null when nothing should fold: still running, failed, or no work before the answer
 * (no tool step and no follow-up revision).
 */
export function workFold(transcript: TranscriptState): WorkFold | null {
  if (transcript.status !== 'completed') return null;
  const blocks = transcript.blocks;
  // The answer is the last text block that a revision did not replace and that is not a
  // working note before tool calls.
  const excluded = notAnswerBlocks(transcript);
  let answerIndex = -1;
  for (let i = blocks.length - 1; i >= 0; i--) {
    const block = blocks[i];
    if (block.kind === 'text' && !excluded.has(block.id)) {
      answerIndex = i;
      break;
    }
  }
  if (answerIndex <= 0) return null;
  const folded = blocks.slice(0, answerIndex);
  const stepCount = folded.filter(b => b.kind === 'step').length;
  const revisions = folded.filter(b => b.kind === 'revision');
  if (stepCount === 0 && revisions.length === 0) return null;

  const sources = new Set(transcript.passages.map(p => p.path)).size;
  const rechecked = revisions.reduce((sum, b) => sum + (b.kind === 'revision' ? b.flagged : 0), 0);
  const parts = [transcript.durationMs !== null ? `Worked for ${seconds(transcript.durationMs)}` : 'Worked'];
  if (stepCount > 0) parts.push(`${stepCount} ${stepCount === 1 ? 'step' : 'steps'}`);
  if (sources > 0) parts.push(`${sources} ${sources === 1 ? 'source' : 'sources'}`);
  if (rechecked > 0) parts.push(`re-checked ${rechecked} ${rechecked === 1 ? 'statement' : 'statements'}`);
  return { answerIndex, label: parts.join(' · ') };
}
