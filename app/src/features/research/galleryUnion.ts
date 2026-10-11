/**
 * Snippets in the visuals gallery. The gallery's records come from two
 * stores (generated visuals and snippets); they are merged here, in the
 * frontend only, into one list of entries with one kind filter, one sort
 * and one count. Pure module, unit-tested with Node
 * (`app/tests/galleryUnion.test.ts`).
 */

import type { VisualSummary } from '../visuals/model.ts';
import { compareVisuals } from '../visuals/model.ts';
import type { VisualKind } from '../visuals/extract.ts';
import type { Snippet } from './types.ts';

/** One card of the gallery. */
export type GalleryEntry =
  | { kind: VisualKind; key: string; visual: VisualSummary }
  | { kind: 'snippet'; key: string; snippet: Snippet };

/** Kind filter of the gallery including snippets. */
export type GalleryFilter = 'all' | VisualKind | 'snippet';

export function visualEntry(visual: VisualSummary): GalleryEntry {
  return { kind: visual.kind, key: `visual:${visual.rootId}`, visual };
}

export function snippetEntry(snippet: Snippet): GalleryEntry {
  return { kind: 'snippet', key: `snippet:${snippet.id}`, snippet };
}

function updatedAt(entry: GalleryEntry): string {
  return entry.kind === 'snippet' ? entry.snippet.updatedAt : entry.visual.updatedAt;
}

function pinned(entry: GalleryEntry): boolean {
  return entry.kind !== 'snippet' && entry.visual.pinned;
}

/**
 * Pinned visuals first (snippets have no pin), then most recently changed;
 * two visuals keep the gallery's own order, ties break on the key.
 */
export function compareEntries(a: GalleryEntry, b: GalleryEntry): number {
  if (a.kind !== 'snippet' && b.kind !== 'snippet') return compareVisuals(a.visual, b.visual);
  if (pinned(a) !== pinned(b)) return pinned(a) ? -1 : 1;
  const ta = updatedAt(a);
  const tb = updatedAt(b);
  if (ta !== tb) return ta < tb ? 1 : -1;
  return a.key < b.key ? -1 : a.key > b.key ? 1 : 0;
}

/** Visuals and snippets as one sorted list. */
export function mergeGallery(visuals: readonly VisualSummary[], snippets: readonly Snippet[]): GalleryEntry[] {
  return [...visuals.map(visualEntry), ...snippets.map(snippetEntry)].sort(compareEntries);
}

/** Entries of one kind (all for "all"), sorted. */
export function filterEntries(entries: readonly GalleryEntry[], filter: GalleryFilter): GalleryEntry[] {
  return [...(filter === 'all' ? entries : entries.filter(e => e.kind === filter))].sort(compareEntries);
}

/** How many entries of each kind there are (kinds with none left out). */
export function entryCounts(entries: readonly GalleryEntry[]): Partial<Record<GalleryFilter, number>> {
  const counts: Partial<Record<GalleryFilter, number>> = {};
  for (const e of entries) counts[e.kind] = (counts[e.kind] ?? 0) + 1;
  return counts;
}

/** Whether a snippet matches gallery search words (title, note, tags, text, file). */
export function snippetMatches(snippet: Snippet, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return true;
  const hay = [snippet.title, snippet.note, snippet.tags.join(' '), snippet.text, snippet.fileName].join(' ').toLowerCase();
  return words.every(w => hay.includes(w));
}

/** The loaded entries with one snippet replaced (or removed when `next` is null). */
export function replaceSnippet(entries: readonly GalleryEntry[], id: string, next: Snippet | null): GalleryEntry[] {
  const rest = entries.filter(e => !(e.kind === 'snippet' && e.snippet.id === id));
  return (next ? [...rest, snippetEntry(next)] : rest).sort(compareEntries);
}
