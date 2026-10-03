/**
 * Long-term memory as the Settings → Memory page shows it: parsing the backend's
 * `MemoryRecord` (camelCase, from `shodh_rag::user_memory`), grouping by class,
 * strength display and building edits. Pure functions only, so they run under
 * `node --test` without React or Tauri.
 */

/** One memory with its live dynamics (mirror of `MemoryRecord`). */
export interface MemoryRecord {
  id: string;
  class: string;
  classLabel: string;
  text: string;
  subject: string | null;
  /** Property values as written (raw ontology values), for editing. */
  properties: Record<string, unknown>;
  /** Property values in canonical text form, for display. */
  values: Record<string, string[]>;
  /** `global` or `workspace:<source id>`. */
  scope: string;
  /** `settings://memory` or `conversation://<conversation>/turn/<run>`. */
  source: string;
  conversationId: string | null;
  extractor: string;
  confidence: number;
  validFrom: string;
  validTo: string | null;
  supersededBy: string | null;
  expiresAt: string | null;
  createdAt: string;
  /** Recall strength in [0, 1], decayed to now (1 when pinned). */
  strength: number;
  importance: number;
  useCount: number;
  lastUsedAt: string | null;
  pinned: boolean;
  current: boolean;
  expired: boolean;
}

/** What to store (mirror of `MemoryContent`, tagged by `kind`). */
export type MemoryContent =
  | { kind: 'note'; text: string }
  | { kind: 'fact'; class: string; subject: { id: string } | null; properties: Record<string, unknown> };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function optString(value: unknown): string | null | undefined {
  if (value === null || value === undefined) return null;
  return typeof value === 'string' ? value : undefined;
}

/** Validate one memory from the backend; `null` when malformed. */
export function parseMemory(value: unknown): MemoryRecord | null {
  if (!isRecord(value)) return null;
  const v = value;
  const strings = ['id', 'class', 'classLabel', 'text', 'scope', 'source', 'extractor', 'validFrom', 'createdAt'] as const;
  if (strings.some(k => typeof v[k] !== 'string')) return null;
  const numbers = ['confidence', 'strength', 'importance', 'useCount'] as const;
  if (numbers.some(k => typeof v[k] !== 'number' || !Number.isFinite(v[k] as number))) return null;
  const booleans = ['pinned', 'current', 'expired'] as const;
  if (booleans.some(k => typeof v[k] !== 'boolean')) return null;
  const optional = ['subject', 'conversationId', 'validTo', 'supersededBy', 'expiresAt', 'lastUsedAt'] as const;
  const opt: Record<string, string | null> = {};
  for (const k of optional) {
    const parsed = optString(v[k]);
    if (parsed === undefined) return null;
    opt[k] = parsed;
  }
  if (!isRecord(v.properties) || !isRecord(v.values)) return null;
  const values: Record<string, string[]> = {};
  for (const [name, list] of Object.entries(v.values)) {
    if (!Array.isArray(list) || list.some(x => typeof x !== 'string')) return null;
    values[name] = list as string[];
  }
  return {
    id: v.id as string,
    class: v.class as string,
    classLabel: v.classLabel as string,
    text: v.text as string,
    subject: opt.subject,
    properties: v.properties,
    values,
    scope: v.scope as string,
    source: v.source as string,
    conversationId: opt.conversationId,
    extractor: v.extractor as string,
    confidence: v.confidence as number,
    validFrom: v.validFrom as string,
    validTo: opt.validTo,
    supersededBy: opt.supersededBy,
    expiresAt: opt.expiresAt,
    createdAt: v.createdAt as string,
    strength: v.strength as number,
    importance: v.importance as number,
    useCount: v.useCount as number,
    lastUsedAt: opt.lastUsedAt,
    pinned: v.pinned as boolean,
    current: v.current as boolean,
    expired: v.expired as boolean,
  };
}

/** Validate a list of memories; malformed entries are dropped. */
export function parseMemories(value: unknown): MemoryRecord[] {
  if (!Array.isArray(value)) return [];
  return value.map(parseMemory).filter((m): m is MemoryRecord => m !== null);
}

/** Classes in display order; any other class follows, alphabetically. */
export const CLASS_ORDER: readonly string[] = [
  'Preference',
  'Person',
  'Organization',
  'Project',
  'Decision',
  'Concept',
  'Procedure',
  'Task',
  'Event',
  'Episode',
  'Note',
];

/** Plural heading of a class group. */
export function groupHeading(classId: string, label: string): string {
  const plural: Record<string, string> = {
    Preference: 'Preferences',
    Person: 'People',
    Organization: 'Organizations',
    Project: 'Projects',
    Decision: 'Decisions',
    Concept: 'Concepts',
    Procedure: 'Procedures',
    Task: 'Tasks',
    Event: 'Events',
    Episode: 'Episodes',
    Note: 'Notes',
  };
  return plural[classId] ?? label;
}

export interface MemoryGroup {
  classId: string;
  heading: string;
  memories: MemoryRecord[];
}

/**
 * Groups memories by class in [`CLASS_ORDER`]. Within a group: pinned first, then
 * strongest, then most recent.
 */
export function groupByClass(memories: readonly MemoryRecord[]): MemoryGroup[] {
  const groups = new Map<string, MemoryRecord[]>();
  for (const memory of memories) {
    const list = groups.get(memory.class);
    if (list) list.push(memory);
    else groups.set(memory.class, [memory]);
  }
  const rank = (classId: string) => {
    const i = CLASS_ORDER.indexOf(classId);
    return i < 0 ? CLASS_ORDER.length : i;
  };
  return [...groups.entries()]
    .sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b))
    .map(([classId, list]) => ({
      classId,
      heading: groupHeading(classId, list[0].classLabel),
      memories: [...list].sort(
        (a, b) =>
          Number(b.pinned) - Number(a.pinned) ||
          b.strength - a.strength ||
          b.validFrom.localeCompare(a.validFrom),
      ),
    }));
}

export type StrengthLevel = 'pinned' | 'strong' | 'fading' | 'faint';

export interface StrengthDisplay {
  /** 0–100, for the bar's width and `aria-valuenow`. */
  percent: number;
  level: StrengthLevel;
  /** Short text next to the bar and in its accessible value. */
  label: string;
}

/** Strength levels: strong ≥ 60 %, fading ≥ 25 %, faint below. */
export function strengthDisplay(strength: number, pinned: boolean): StrengthDisplay {
  const clamped = Number.isFinite(strength) ? Math.min(1, Math.max(0, strength)) : 0;
  const percent = Math.round(clamped * 100);
  if (pinned) return { percent: 100, level: 'pinned', label: 'Pinned · does not fade' };
  const level: StrengthLevel = clamped >= 0.6 ? 'strong' : clamped >= 0.25 ? 'fading' : 'faint';
  const word = level === 'strong' ? 'Strong' : level === 'fading' ? 'Fading' : 'Faint';
  return { percent, level, label: `${word} · ${percent}%` };
}

/** "today", "yesterday", "5 days ago", "3 weeks ago", "4 months ago", "2 years ago". */
export function relativeTime(iso: string | null, now: Date): string {
  if (!iso) return 'never';
  const then = new Date(iso);
  if (Number.isNaN(then.getTime())) return 'unknown';
  const startOf = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((startOf(now) - startOf(then)) / 86_400_000);
  if (days <= 0) return 'today';
  if (days === 1) return 'yesterday';
  if (days < 14) return `${days} days ago`;
  if (days < 60) return `${Math.floor(days / 7)} weeks ago`;
  if (days < 730) return `${Math.floor(days / 30)} months ago`;
  return `${Math.floor(days / 365)} years ago`;
}

/** "Oct 3, 2026" in local time. */
export function formatDate(iso: string | null): string {
  if (!iso) return '';
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

/** Where a memory is visible, in words. `sourceName` resolves a source id. */
export function scopeLabel(scope: string, sourceName: (id: string) => string | undefined): string {
  if (scope === 'global') return 'All conversations';
  const id = scope.startsWith('workspace:') ? scope.slice('workspace:'.length) : scope;
  return `Conversations about ${sourceName(id) ?? 'a removed source'}`;
}

/** "preferenceTopic" → "Preference topic". */
export function propertyLabel(name: string): string {
  const words = name.replace(/([a-z0-9])([A-Z])/g, '$1 $2').toLowerCase();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

export interface EditField {
  name: string;
  label: string;
  value: string;
  multiline: boolean;
}

/**
 * The fields the editor offers: a note's text, or each text-valued property of a fact.
 * Other values (people, dates as references, lists) are kept unchanged on save.
 */
export function editableFields(memory: MemoryRecord): EditField[] {
  if (memory.class === 'Note') {
    const text = typeof memory.properties.noteText === 'string' ? memory.properties.noteText : memory.text;
    return [{ name: 'noteText', label: 'Note', value: text, multiline: true }];
  }
  return Object.entries(memory.properties)
    .filter(([, value]) => typeof value === 'string')
    .map(([name, value]) => ({
      name,
      label: propertyLabel(name),
      value: value as string,
      multiline: (value as string).length > 80,
    }));
}

/**
 * The complete new fact for an edit (`memory_update` replaces the whole fact): edited
 * fields replace their values, everything else is kept. `null` when nothing changed or
 * a field was emptied.
 */
export function contentFromEdit(memory: MemoryRecord, edited: Record<string, string>): MemoryContent | null {
  const fields = editableFields(memory);
  const changed = fields.some(f => (edited[f.name] ?? f.value).trim() !== f.value.trim());
  if (!changed) return null;
  if (fields.some(f => (edited[f.name] ?? f.value).trim() === '')) return null;
  if (memory.class === 'Note') {
    return { kind: 'note', text: (edited.noteText ?? '').trim() };
  }
  const properties: Record<string, unknown> = { ...memory.properties };
  for (const field of fields) {
    properties[field.name] = (edited[field.name] ?? field.value).trim();
  }
  return {
    kind: 'fact',
    class: memory.class,
    subject: memory.subject ? { id: memory.subject } : null,
    properties,
  };
}

/** Case-insensitive match of a memory against the search box (text, class, values). */
export function matchesFilter(memory: MemoryRecord, filter: string): boolean {
  const needle = filter.trim().toLowerCase();
  if (!needle) return true;
  const haystack = [memory.text, memory.classLabel, ...Object.values(memory.values).flat()]
    .join(' ')
    .toLowerCase();
  return needle.split(/\s+/).every(word => haystack.includes(word));
}

/** Default export file name, e.g. `shodh-memories-2026-10-03.json`. */
export function exportFileName(now: Date): string {
  const y = now.getFullYear();
  const m = String(now.getMonth() + 1).padStart(2, '0');
  const d = String(now.getDate()).padStart(2, '0');
  return `shodh-memories-${y}-${m}-${d}.json`;
}
