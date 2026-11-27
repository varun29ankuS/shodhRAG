import { stepKind } from './workSummary.ts';
import type { TranscriptState } from './reducer';

/**
 * What a finished run did, for the Summary of its run view (`RunPanel`): the sources it
 * read (each opens its passage), what it changed, the commands it ran, and the parts of
 * its plan it could not do. Only from the run's own record; nothing is guessed.
 */
export interface RunSummary {
  /** One per source file, by its first passage's number. */
  sources: Array<{ n: number; label: string }>;
  /** The steps that wrote or changed files, as the run named them. */
  changed: string[];
  /** The commands it ran, as the run named them. */
  commands: string[];
  /** Plan items it found nothing for, or could not do (with why). */
  open: string[];
}

function unique(values: readonly string[]): string[] {
  return [...new Set(values.filter(v => v.trim().length > 0))];
}

/** The summary of `transcript`, or null while it runs or when it did nothing to report. */
export function runSummary(transcript: TranscriptState): RunSummary | null {
  if (transcript.status !== 'completed') return null;
  const seen = new Set<string>();
  const sources: RunSummary['sources'] = [];
  for (const passage of [...transcript.passages].sort((a, b) => a.n - b.n)) {
    if (seen.has(passage.path)) continue;
    seen.add(passage.path);
    sources.push({ n: passage.n, label: passage.file });
  }
  const steps = Object.values(transcript.steps)
    .filter(step => step.parentId === null && step.status === 'done')
    .sort((a, b) => a.startedAtMs - b.startedAtMs);
  const changed = unique(steps.filter(step => stepKind(step.tool) === 'edit').map(step => step.label));
  const commands = unique(steps.filter(step => stepKind(step.tool) === 'command').map(step => step.label));
  const open = unique((transcript.plan ?? []).flatMap(item => {
    if (item.status === 'blocked') return [item.reason ? `${item.text}: ${item.reason}` : item.text];
    if (item.need && item.coverage === 'missing') return [`${item.text}: not found`];
    return [];
  }));
  if (sources.length + changed.length + commands.length + open.length === 0) return null;
  return { sources, changed, commands, open };
}
