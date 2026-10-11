/**
 * Gallery records as the backend returns them, the focus target of a
 * record, and the pure list operations of the gallery (filter, sort,
 * optimistic changes).
 *
 * Pure module (type-only imports apart from target builders), unit-tested
 * with Node (`app/tests/visualGallery.test.ts`).
 */

import type { FocusParamValue, FocusTarget } from '../focus/focusTypes.ts';
import { readParamValues } from '../focus/threadStore.ts';
import { isVisualKind, KIND_NOUN, markdownTableRows } from './extract.ts';
import type { VisualKind } from './extract.ts';

export type VisualAuthor = 'capture' | 'user' | 'agent';

/** One stored version (`VisualRecord` in Rust, camelCase). */
export interface VisualRecord {
  id: string;
  rootId: string;
  parentId: string | null;
  version: number;
  conversationId: string;
  messageId: string | null;
  threadId: string | null;
  turnId: string | null;
  kind: VisualKind;
  title: string;
  source: string;
  params: Record<string, unknown>;
  contentHash: string;
  pinned: boolean;
  note: string;
  instruction: string | null;
  createdBy: VisualAuthor;
  createdAt: string;
  updatedAt: string;
}

/** A gallery card: the latest version of a visual. */
export interface VisualSummary extends VisualRecord {
  versionCount: number;
  firstCreatedAt: string;
}

export interface VisualVersionInfo {
  id: string;
  version: number;
  createdBy: VisualAuthor;
  instruction: string | null;
  createdAt: string;
}

export interface VisualDetail {
  visual: VisualRecord;
  versions: VisualVersionInfo[];
}

export interface VisualPage {
  items: VisualSummary[];
  total: number;
}

/** Where captured visuals came from (`VisualOrigin`). */
export interface VisualOrigin {
  conversationId: string;
  messageId: string | null;
  threadId: string | null;
  turnId: string | null;
}

/** Slider positions kept with a version (`params.values`). */
export function paramValuesOf(params: unknown): FocusParamValue[] {
  if (typeof params !== 'object' || params === null || Array.isArray(params)) return [];
  return readParamValues((params as { values?: unknown }).values);
}

/** The params stored with a version for these slider positions. */
export function paramsFor(values: readonly FocusParamValue[]): Record<string, unknown> {
  return values.length > 0 ? { values: readParamValues(values) } : {};
}

/** The focus target that draws a stored version, or null when it cannot be drawn. */
export function recordTarget(record: Pick<VisualRecord, 'kind' | 'title' | 'source' | 'params'>): FocusTarget | null {
  const label = record.title.trim() || KIND_NOUN[record.kind];
  switch (record.kind) {
    case 'mermaid':
      return { kind: 'mermaid', label, source: record.source };
    case 'chart':
      return { kind: 'chart', label, source: record.source };
    case 'svg':
      return { kind: 'svg', label, source: record.source };
    case 'plot':
      return { kind: 'plot', label, source: record.source, values: paramValuesOf(record.params) };
    case 'simulation':
      return { kind: 'simulation', label, source: record.source, values: paramValuesOf(record.params) };
    case 'equation':
      return { kind: 'equation', label, tex: record.source };
    case 'table': {
      const rows = markdownTableRows(record.source);
      return rows.length > 0 ? { kind: 'table', label, rows } : null;
    }
    default:
      return null;
  }
}

/** A gallery visual as the focus pop-out carries it. */
export interface VisualRef {
  /** The version shown. */
  id: string;
  rootId: string;
  version: number;
  conversationId: string;
  messageId: string | null;
}

export function visualRef(record: VisualRecord): VisualRef {
  return {
    id: record.id,
    rootId: record.rootId,
    version: record.version,
    conversationId: record.conversationId,
    messageId: record.messageId,
  };
}

/** Kind filter of the gallery ("all" or one kind). */
export type KindFilter = 'all' | VisualKind;

export function isKindFilter(value: unknown): value is KindFilter {
  return value === 'all' || isVisualKind(value);
}

/** Pinned first, then most recently changed, then newest version first. */
export function compareVisuals(a: VisualSummary, b: VisualSummary): number {
  if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;
  if (a.updatedAt !== b.updatedAt) return a.updatedAt < b.updatedAt ? 1 : -1;
  if (a.createdAt !== b.createdAt) return a.createdAt < b.createdAt ? 1 : -1;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

export function sortVisuals(items: readonly VisualSummary[]): VisualSummary[] {
  return [...items].sort(compareVisuals);
}

/** Cards of one kind (all for "all"), sorted. */
export function filterVisuals(items: readonly VisualSummary[], kind: KindFilter): VisualSummary[] {
  return sortVisuals(kind === 'all' ? items : items.filter(v => v.kind === kind));
}

/** How many cards of each kind there are (kinds with none left out). */
export function kindCounts(items: readonly VisualSummary[]): Partial<Record<VisualKind, number>> {
  const counts: Partial<Record<VisualKind, number>> = {};
  for (const item of items) counts[item.kind] = (counts[item.kind] ?? 0) + 1;
  return counts;
}

/** A change to apply to the loaded cards before the backend confirms it. */
export type GalleryChange =
  | { type: 'pinned'; rootId: string; pinned: boolean; at: string }
  | { type: 'renamed'; rootId: string; title: string; at: string }
  | { type: 'noted'; rootId: string; note: string; at: string }
  | { type: 'removed'; rootId: string }
  | { type: 'replaced'; card: VisualSummary };

/** The loaded cards with `change` applied, sorted. */
export function applyChange(items: readonly VisualSummary[], change: GalleryChange): VisualSummary[] {
  switch (change.type) {
    case 'removed':
      return items.filter(v => v.rootId !== change.rootId);
    case 'replaced': {
      const rest = items.filter(v => v.rootId !== change.card.rootId);
      return sortVisuals([...rest, change.card]);
    }
    case 'pinned':
      return sortVisuals(items.map(v => (v.rootId === change.rootId ? { ...v, pinned: change.pinned, updatedAt: change.at } : v)));
    case 'renamed':
      return sortVisuals(items.map(v => (v.rootId === change.rootId ? { ...v, title: change.title, updatedAt: change.at } : v)));
    case 'noted':
      return sortVisuals(items.map(v => (v.rootId === change.rootId ? { ...v, note: change.note, updatedAt: change.at } : v)));
    default:
      return [...items];
  }
}

/** A card for a detail the backend returned after a change (its latest version). */
export function cardFromDetail(detail: VisualDetail, firstCreatedAt?: string): VisualSummary {
  const latest = detail.versions.reduce((max, v) => (v.version > max ? v.version : max), detail.visual.version);
  return {
    ...detail.visual,
    versionCount: latest,
    firstCreatedAt: firstCreatedAt ?? detail.versions[0]?.createdAt ?? detail.visual.createdAt,
  };
}

/** "Diagram · v3" style meta line parts. */
export function versionLabel(version: number, count: number): string {
  return count > 1 ? `v${version} of ${count}` : 'v1';
}
