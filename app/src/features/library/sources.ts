/**
 * Library sources: the folders (and, later, connected accounts) Shodh
 * indexes. Persistence, normalisation of what an indexing run returns, and
 * grouping by source kind.
 *
 * Pure module (no runtime imports) so it is unit-tested directly with Node
 * (`app/tests/librarySources.test.ts`).
 */

import type { FileFailure } from './fileTree';

export type SourceStatus = 'ready' | 'indexing' | 'error' | 'interrupted';

export interface LibrarySource {
  id: string;
  name: string;
  path: string;
  type: 'documents';
  fileCount: number;
  /** When the source was last indexed successfully (or added, before that). */
  indexedAt: string;
  status: SourceStatus;
  /** Included when answering in Ask. */
  selected: boolean;
  progress?: number;
  currentFile?: string;
  processedCount?: number;
  /** Files the last indexing run could not index, with reasons. */
  failures?: FileFailure[];
  /** Why the last indexing run failed as a whole. */
  lastError?: string;
  /** Why the folder could not be kept in sync (this session only). */
  syncError?: string;
}

export const SOURCES_STORAGE_KEY = 'indexedSources';

const STATUSES: readonly SourceStatus[] = ['ready', 'indexing', 'error', 'interrupted'];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function toFailures(value: unknown): FileFailure[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter(isRecord)
    .filter(f => typeof f.file === 'string' && f.file.length > 0)
    .map(f => ({ file: f.file as string, reason: typeof f.reason === 'string' ? f.reason : '' }));
}

/**
 * Sources saved by this or an older build. Malformed entries are dropped;
 * a source saved mid-index belongs to a run that died with the previous
 * session, so it is marked 'interrupted' (and would otherwise absorb the
 * progress events of the next run).
 */
export function parseStoredSources(raw: string | null): LibrarySource[] {
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  const out: LibrarySource[] = [];
  const ids = new Set<string>();
  for (const item of parsed) {
    if (!isRecord(item) || typeof item.id !== 'string' || typeof item.path !== 'string' || ids.has(item.id)) continue;
    ids.add(item.id);
    const status = STATUSES.includes(item.status as SourceStatus) ? (item.status as SourceStatus) : 'ready';
    const failures = toFailures(item.failures);
    out.push({
      id: item.id,
      name: typeof item.name === 'string' && item.name ? item.name : item.path,
      path: item.path,
      type: 'documents',
      fileCount: typeof item.fileCount === 'number' && Number.isFinite(item.fileCount) ? item.fileCount : 0,
      indexedAt: typeof item.indexedAt === 'string' ? item.indexedAt : '',
      status: status === 'indexing' ? 'interrupted' : status,
      selected: item.selected !== false,
      ...(failures.length > 0 ? { failures } : {}),
      ...(typeof item.lastError === 'string' && item.lastError ? { lastError: item.lastError } : {}),
    });
  }
  return out;
}

/** The persisted form: live progress and sync fields are not stored. */
export function serializeSources(sources: readonly LibrarySource[]): string {
  return JSON.stringify(
    sources.map(({ progress: _p, currentFile: _c, processedCount: _n, syncError: _s, ...rest }) => rest),
  );
}

/** A folder the backend keeps in sync (`sync_folder_sources`). */
export interface FolderSourceRef {
  id: string;
  path: string;
}

/**
 * The sources to keep in sync: those whose index is complete. A source being
 * indexed is left out until its run ends, so a sync never races it.
 */
export function syncedFolders(sources: readonly LibrarySource[]): FolderSourceRef[] {
  return sources.filter(s => s.status === 'ready').map(s => ({ id: s.id, path: s.path }));
}

/** The `folder-sync` event: one sync of a folder source. */
export interface FolderSyncOutcome {
  sourceId: string;
  at: string;
  indexed?: number;
  removed?: number;
  files?: number;
  failures?: unknown;
  error?: string | null;
}

/**
 * A source after a sync: its file count and failures as the folder has them
 * now, `indexedAt` moved when files were indexed or removed, or the reason it
 * could not be synced.
 */
export function applyFolderSync(source: LibrarySource, outcome: FolderSyncOutcome): LibrarySource {
  if (source.id !== outcome.sourceId || source.status !== 'ready') return source;
  if (outcome.error) return { ...source, syncError: outcome.error };
  const failures = toFailures(outcome.failures);
  const changed = (outcome.indexed ?? 0) + (outcome.removed ?? 0) > 0;
  const { syncError: _s, failures: _f, ...rest } = source;
  return {
    ...rest,
    fileCount: Math.max(0, (outcome.files ?? source.fileCount) - failures.length),
    ...(changed ? { indexedAt: outcome.at } : {}),
    ...(failures.length > 0 ? { failures } : {}),
  };
}

export interface IndexingOutcome {
  filesProcessed: number;
  totalChunks: number;
  failures: FileFailure[];
}

/** Normalise the result of `link_folder_enhanced` / `index_single_file` (snake_case `IndexingResult`). */
export function readIndexingResult(result: unknown): IndexingOutcome {
  if (!isRecord(result)) return { filesProcessed: 0, totalChunks: 0, failures: [] };
  const num = (...keys: string[]) => {
    for (const key of keys) {
      const v = result[key];
      if (typeof v === 'number' && Number.isFinite(v)) return v;
    }
    return 0;
  };
  let failures = toFailures(result.failures);
  // Older builds only reported paths.
  if (failures.length === 0 && Array.isArray(result.failed_files)) {
    failures = result.failed_files
      .filter((f): f is string => typeof f === 'string' && f.length > 0)
      .map(file => ({ file, reason: '' }));
  }
  return {
    filesProcessed: num('files_processed', 'filesProcessed', 'file_count', 'fileCount'),
    totalChunks: num('total_chunks', 'totalChunks'),
    failures,
  };
}

/** Progress of a running index, clamped to 0–100. */
export function progressPercent(source: Pick<LibrarySource, 'progress'>): number {
  return Math.max(0, Math.min(100, Math.round(source.progress ?? 0)));
}

/**
 * Kinds of source the Library lists, in display order. Only kinds that have
 * sources are shown, so new kinds (mail, drive connectors) slot in here
 * without the Library ever showing an empty placeholder.
 */
export const SOURCE_KINDS = [
  { id: 'folders', label: 'Folders', matches: (_s: LibrarySource) => true },
] as const;

export type SourceKindId = typeof SOURCE_KINDS[number]['id'];

export function groupSourcesByKind(sources: readonly LibrarySource[]): { id: SourceKindId; label: string; sources: LibrarySource[] }[] {
  const remaining = [...sources];
  const groups: { id: SourceKindId; label: string; sources: LibrarySource[] }[] = [];
  for (const kind of SOURCE_KINDS) {
    const mine = remaining.filter(s => kind.matches(s));
    if (mine.length === 0) continue;
    groups.push({ id: kind.id, label: kind.label, sources: mine });
    for (const s of mine) remaining.splice(remaining.indexOf(s), 1);
  }
  return groups;
}
