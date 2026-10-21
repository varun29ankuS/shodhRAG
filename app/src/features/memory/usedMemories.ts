/**
 * "Used 2 memories" under an answer: the memories put in front of its question, read live
 * by id (`memory_used_lookup`). The answer keeps their ids only (`TranscriptState.memoryIds`),
 * so a memory forgotten since shows as "a forgotten memory", without its words, and an
 * edited one as it is now. Pure functions only, so they run under `node --test` without
 * React or Tauri.
 */
import { formatDate, parseMemory } from './model.ts';
import type { MemoryRecord } from './model.ts';

/** One memory of an answer, as `memory_used_lookup` returns it. */
export interface UsedMemory {
  id: string;
  /** The memory now; null when it was forgotten. */
  memory: MemoryRecord | null;
  /** The user marked it not relevant to this answer. */
  notRelevant: boolean;
  /** It was given as a core memory: in front of every question, whatever it asked. */
  alwaysIncluded: boolean;
}

/** One row of the "Used N memories" list. */
export interface UsedMemoryRow {
  id: string;
  /** Its words now; null when forgotten. */
  text: string | null;
  /** "Note", "Preference", ... (empty when forgotten). */
  classLabel: string;
  /** "since 3 Mar 2026" (empty when forgotten). */
  since: string;
  /** "Global" or the workspace's name (empty when forgotten). */
  scope: string;
  /** It was replaced by a newer version since the answer (edited, or a newer fact). */
  replaced: boolean;
  /** "Outdated" can open its editor: it still exists and is the current version. */
  canEdit: boolean;
  /** "Not relevant" was recorded for this answer. */
  notRelevant: boolean;
  /** A core memory: "always included" (kept for a forgotten one too). */
  alwaysIncluded: boolean;
}

/** What a forgotten memory shows instead of its words. */
export const FORGOTTEN_MEMORY = 'a forgotten memory';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** The chip's words: "Used 1 memory", "Used 3 memories"; null (no chip) for none. */
export function chipText(count: number): string | null {
  if (!Number.isInteger(count) || count <= 0) return null;
  return `Used ${count} ${count === 1 ? 'memory' : 'memories'}`;
}

/**
 * The lookup's answer for `ids`, validated and in the order of `ids`: an id the backend
 * did not return, or returned malformed, counts as forgotten (its words are never guessed).
 */
export function parseUsedMemories(value: unknown, ids: readonly string[]): UsedMemory[] {
  const byId = new Map<string, UsedMemory>();
  if (Array.isArray(value)) {
    for (const entry of value) {
      if (!isRecord(entry) || typeof entry.id !== 'string') continue;
      byId.set(entry.id, {
        id: entry.id,
        memory: entry.memory === null ? null : parseMemory(entry.memory),
        notRelevant: entry.notRelevant === true,
        alwaysIncluded: entry.alwaysIncluded === true,
      });
    }
  }
  return ids.map(id => byId.get(id) ?? { id, memory: null, notRelevant: false, alwaysIncluded: false });
}

/** Where a memory applies, in one or two words: "Global" or its workspace's name. */
export function shortScope(scope: string, workspaceName: (id: string) => string | undefined): string {
  if (scope === 'global') return 'Global';
  const id = scope.startsWith('workspace:') ? scope.slice('workspace:'.length) : scope;
  return workspaceName(id) ?? 'A deleted workspace';
}

/** The row of one memory. */
export function rowOf(used: UsedMemory, workspaceName: (id: string) => string | undefined): UsedMemoryRow {
  const m = used.memory;
  if (!m) {
    return {
      id: used.id,
      text: null,
      classLabel: '',
      since: '',
      scope: '',
      replaced: false,
      canEdit: false,
      notRelevant: used.notRelevant,
      alwaysIncluded: used.alwaysIncluded,
    };
  }
  const since = formatDate(m.validFrom);
  return {
    id: used.id,
    text: m.text,
    classLabel: m.classLabel,
    since: since ? `since ${since}` : '',
    scope: shortScope(m.scope, workspaceName),
    replaced: !m.current && m.supersededBy !== null,
    canEdit: m.current,
    notRelevant: used.notRelevant,
    alwaysIncluded: used.alwaysIncluded,
  };
}

/** The heading of Settings, Memory's core list: "Always included in answers (3)". */
export function coreHeading(count: number): string {
  return `Always included in answers (${Number.isInteger(count) && count > 0 ? count : 0})`;
}

/** `used` with `id` marked not relevant (after the backend recorded it). */
export function markedNotRelevant(used: readonly UsedMemory[], id: string): UsedMemory[] {
  return used.map(u => (u.id === id ? { ...u, notRelevant: true } : u));
}
