/**
 * Builders of focus targets from what each surface has at hand, with a
 * short human label for each. Payloads are capped so a thread record stays
 * small. Pure module, unit-tested with Node (`app/tests/focusContext.test.ts`).
 */

import type { FocusSourceHit, FocusTarget, FocusTaskSnapshot } from './focusTypes.ts';
import { MAX_TARGET_CHARS } from './threadStore.ts';

const LABEL_CHARS = 60;
/** Rows and columns of a table kept with a thread. */
const MAX_ROWS = 400;
const MAX_COLUMNS = 40;
const MAX_CELL = 500;

function short(text: string, max = LABEL_CHARS): string {
  const flat = text.replace(/\s+/g, ' ').trim();
  const chars = Array.from(flat);
  return chars.length > max ? `${chars.slice(0, max - 1).join('')}…` : flat;
}

function cap(text: string, max = MAX_TARGET_CHARS): string {
  const chars = Array.from(text);
  return chars.length > max ? chars.slice(0, max).join('') : text;
}

const DIAGRAM_NAMES: Record<string, string> = {
  flowchart: 'Flowchart',
  graph: 'Flowchart',
  sequencediagram: 'Sequence diagram',
  classdiagram: 'Class diagram',
  statediagram: 'State diagram',
  'statediagram-v2': 'State diagram',
  erdiagram: 'Entity relationship diagram',
  gantt: 'Gantt chart',
  pie: 'Pie chart',
  journey: 'User journey',
  gitgraph: 'Git graph',
  mindmap: 'Mind map',
  timeline: 'Timeline',
};

export function mermaidTarget(source: string): FocusTarget {
  const lines = source.split('\n').map(l => l.trim()).filter(l => l && !l.startsWith('%%'));
  const title = /^\s*title\s+(.+)$/im.exec(source)?.[1];
  const keyword = (lines[0] ?? '').split(/\s+/)[0].toLowerCase();
  const name = DIAGRAM_NAMES[keyword] ?? 'Diagram';
  return { kind: 'mermaid', label: title ? short(title) : name, source: cap(source) };
}

export function chartTarget(source: string, title?: string | null): FocusTarget {
  let label = title?.trim() || '';
  if (!label) {
    try {
      const parsed = JSON.parse(source) as { title?: unknown };
      if (typeof parsed.title === 'string') label = parsed.title;
    } catch {
      // Not JSON: keep the generic label.
    }
  }
  return { kind: 'chart', label: label ? short(label) : 'Chart', source: cap(source) };
}

export function equationTarget(tex: string): FocusTarget {
  return { kind: 'equation', label: `Equation ${short(tex, 40)}`, tex: cap(tex) };
}

export function tableTarget(rows: readonly (readonly string[])[]): FocusTarget | null {
  const kept = rows
    .filter(r => r.length > 0)
    .slice(0, MAX_ROWS)
    .map(r => r.slice(0, MAX_COLUMNS).map(c => cap(c, MAX_CELL)));
  if (kept.length === 0) return null;
  const header = kept[0].filter(c => c.trim()).join(', ');
  return { kind: 'table', label: header ? `Table: ${short(header, 50)}` : 'Table', rows: kept };
}

/** Images kept by address only; inline data is too large to keep with a thread. */
export function imageTarget(src: string | null | undefined, alt: string | null | undefined): FocusTarget {
  const keep = src && !src.startsWith('data:') ? src : src && src.length <= 200_000 ? src : null;
  return { kind: 'image', label: alt?.trim() ? short(alt) : 'Image', src: keep, alt: alt?.trim() ?? '' };
}

export function sourceTarget(hit: FocusSourceHit, label: string): FocusTarget {
  return {
    kind: 'source',
    label: short(label, 80),
    hit: { ...hit, text: cap(hit.text), snippet: cap(hit.snippet, 1_000) },
  };
}

export function taskTarget(task: FocusTaskSnapshot): FocusTarget {
  return { kind: 'task', label: short(task.title) || 'Task', task: { ...task, notes: cap(task.notes) } };
}
