/**
 * Words for Code mode's code map: the status line's chip ("Code map: up to
 * date · 2,341 symbols"), its popover's details, and the symbol jump's rows.
 * Pure functions (tested in tests/codeMap.test.ts); types only from the API.
 */
import type { CodeMapView, SymbolHit } from './api';

function grouped(n: number): string {
  return Math.max(0, Math.round(n)).toLocaleString('en-US');
}

function plural(n: number, word: string): string {
  return `${grouped(n)} ${word}${n === 1 ? '' : 's'}`;
}

const LANGUAGE_NAMES: Record<string, string> = {
  rust: 'Rust',
  typescript: 'TypeScript',
  tsx: 'TSX',
  javascript: 'JavaScript',
  jsx: 'JSX',
  python: 'Python',
  go: 'Go',
};

/** The tone the chip's dot shows. */
export type CodeMapTone = 'good' | 'busy' | 'warn' | 'bad';

/** Whether a view is a code map view the backend sent (events carry the same shape). */
export function readCodeMapView(value: unknown): CodeMapView | null {
  if (typeof value !== 'object' || value === null) return null;
  const v = value as Record<string, unknown>;
  if (typeof v.sourceId !== 'string' || typeof v.name !== 'string') return null;
  if (v.state !== 'idle' && v.state !== 'queued' && v.state !== 'building' && v.state !== 'failed') return null;
  return value as CodeMapView;
}

/** The chip's words. */
export function codeMapChipLabel(view: CodeMapView): string {
  switch (view.state) {
    case 'building': {
      if (view.paused) return 'Code map: paused';
      const total = view.total ?? 0;
      if (total === 0) return 'Code map: listing files';
      return `Code map: ${grouped(Math.min(view.done ?? 0, total))} of ${plural(total, 'file')}`;
    }
    case 'queued':
      return view.map?.completeAt ? `Code map: updating · ${plural(view.map.symbols, 'symbol')}` : 'Code map: starting';
    case 'failed':
      return 'Code map: not working';
    case 'idle':
      if (!view.map?.completeAt) return view.map && view.map.files > 0 ? 'Code map: not finished' : 'Code map: not built';
      return `Code map: up to date · ${plural(view.map.symbols, 'symbol')}`;
  }
}

export function codeMapTone(view: CodeMapView): CodeMapTone {
  if (view.state === 'failed') return 'bad';
  if (view.state === 'building' || view.state === 'queued') return view.paused ? 'warn' : 'busy';
  return view.map?.completeAt ? 'good' : 'warn';
}

/** "3 min ago", "just now", "on 4 Nov" from an RFC 3339 time. */
export function sinceText(iso: string | null | undefined, now: Date): string | null {
  if (!iso) return null;
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return null;
  const seconds = Math.max(0, Math.round((now.getTime() - at.getTime()) / 1000));
  if (seconds < 45) return 'just now';
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return hours === 1 ? '1 hour ago' : `${hours} hours ago`;
  return `on ${at.toLocaleDateString('en-GB', { day: 'numeric', month: 'short' })}`;
}

/** The popover's lines: what is indexed, when, and what the last refresh did. */
export function codeMapDetails(view: CodeMapView, now: Date): string[] {
  const lines: string[] = [];
  const map = view.map;
  if (map && map.files > 0) {
    const languages = map.languages
      .filter(([, n]) => n > 0)
      .sort((a, b) => b[1] - a[1])
      .map(([lang, n]) => `${LANGUAGE_NAMES[lang] ?? lang} ${grouped(n)}`)
      .join(', ');
    lines.push(`Indexed ${plural(map.files, 'file')}${languages ? ` (${languages})` : ''}: ${plural(map.symbols, 'symbol')}, ${plural(map.references, 'use')}.`);
  } else {
    lines.push('Nothing indexed yet.');
  }
  const updated = sinceText(map?.updatedAt, now);
  const checked = sinceText(map?.completeAt, now);
  if (updated) lines.push(`Last change ${updated}${checked && checked !== updated ? `, last checked ${checked}` : ''}.`);
  else if (checked) lines.push(`Last checked ${checked}.`);
  if (map?.partial) lines.push('The project has more files than the map holds; it covers the first 20,000.');
  const run = view.lastRun;
  if (run && !run.stopped) {
    lines.push(
      run.read === 0 && run.removed === 0
        ? `The last check found no changes (${grouped(run.millis)} ms).`
        : `The last check read ${plural(run.read, 'changed file')} and dropped ${plural(run.removed, 'deleted file')} (${grouped(run.millis)} ms).`,
    );
  } else if (run?.stopped) {
    lines.push('The last update was stopped before the end; the rest is read next time.');
  }
  if (view.state === 'failed' && view.error) lines.push(`Error: ${view.error}`);
  return lines;
}

/** A symbol jump row: "method Store::open", and where it is. */
export function symbolRow(hit: SymbolHit): { title: string; where: string } {
  const name = hit.container ? `${hit.container}::${hit.name}` : hit.name;
  return { title: `${hit.kind} ${name}`, where: `${hit.path}:${hit.line}` };
}
