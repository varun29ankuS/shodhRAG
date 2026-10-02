import type { ResponseMetadata, RunRecord, RunStep } from './types';

/** "0.8 s", "3.2 s", "42 s", "1 m 05 s". */
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return '0 s';
  const seconds = ms / 1000;
  if (seconds < 10) return `${seconds.toFixed(1)} s`;
  if (seconds < 60) return `${Math.round(seconds)} s`;
  const whole = Math.round(seconds);
  const m = Math.floor(whole / 60);
  const s = whole % 60;
  return `${m} m ${String(s).padStart(2, '0')} s`;
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

function quote(text: string): string {
  return `“${text}”`;
}

/**
 * The retrieval step, derived after completion from the response itself:
 * the queries the engine reports it dispatched and the passages it returned.
 * Returns null when the response shows no retrieval happened.
 */
export function buildSearchStep(metadata: ResponseMetadata | undefined, passageCount: number): RunStep | null {
  const queries = (metadata?.searchQueriesUsed ?? []).filter(q => typeof q === 'string' && q.trim().length > 0);
  if (queries.length === 0 && passageCount === 0) return null;
  const metaParts: string[] = [];
  if (passageCount > 0) metaParts.push(plural(passageCount, 'passage', 'passages'));
  return {
    id: 'search',
    kind: 'search',
    title: 'Searched',
    detail: queries.length > 0 ? queries.map(quote).join(' · ') : undefined,
    status: 'done',
    meta: metaParts.join(' · ') || undefined,
  };
}

export interface RunSummary {
  headline: string;
  details: string[];
}

/** Collapsed run chip text, built only from recorded data. */
export function summarizeRun(run: RunRecord, metadata: ResponseMetadata | undefined, passageCount: number): RunSummary {
  const elapsed = typeof run.elapsedMs === 'number' ? formatDuration(run.elapsedMs) : null;
  let headline: string;
  switch (run.status) {
    case 'cancelled':
      headline = elapsed ? `Stopped after ${elapsed}` : 'Stopped';
      break;
    case 'failed':
      headline = elapsed ? `Failed after ${elapsed}` : 'Failed';
      break;
    case 'running':
      headline = 'Working';
      break;
    default:
      headline = elapsed ? `Worked for ${elapsed}` : 'Done';
  }

  const details: string[] = [];
  const queries = (metadata?.searchQueriesUsed ?? []).filter(q => typeof q === 'string' && q.trim().length > 0);
  if (queries.length > 0) details.push(`searched ${plural(queries.length, 'query', 'queries')}`);
  if (passageCount > 0) details.push(plural(passageCount, 'passage', 'passages'));
  const toolCount = run.steps.filter(s => s.kind === 'tool').length;
  if (toolCount > 0) details.push(plural(toolCount, 'tool', 'tools'));
  return { headline, details };
}

/**
 * Compact, human-readable form of tool-call arguments. Tool arguments arrive
 * as a JSON string (or object); prefer a `query`-like field when present.
 */
export function describeArguments(args: unknown): string | undefined {
  let value: unknown = args;
  if (typeof args === 'string') {
    const trimmed = args.trim();
    if (!trimmed || trimmed === '{}') return undefined;
    try {
      value = JSON.parse(trimmed);
    } catch {
      return truncate(trimmed, 140);
    }
  }
  if (value === null || value === undefined) return undefined;
  if (typeof value !== 'object') return truncate(String(value), 140);
  const record = value as Record<string, unknown>;
  for (const key of ['query', 'q', 'search', 'question', 'goal', 'task', 'path', 'url']) {
    const v = record[key];
    if (typeof v === 'string' && v.trim()) return truncate(v.trim(), 140);
  }
  const entries = Object.entries(record).filter(([, v]) => v !== null && v !== undefined && v !== '');
  if (entries.length === 0) return undefined;
  return truncate(
    entries.map(([k, v]) => `${k}: ${typeof v === 'string' ? v : JSON.stringify(v)}`).join(', '),
    140,
  );
}

export function truncate(text: string, max: number): string {
  const chars = Array.from(text);
  return chars.length > max ? `${chars.slice(0, max - 1).join('')}…` : text;
}
