/**
 * The Library's folders against what the index holds: the indexed files that
 * no listed folder covers, and the folders they are in. The Library shows
 * them so it never says "nothing added" while answers cite indexed files.
 *
 * Paths compare as Windows paths do: forward slashes, case-insensitively.
 * Pure module so it is unit-tested directly with Node
 * (`app/tests/libraryReconcile.test.ts`).
 */

import { countOf } from '../../lib/plural.ts';

export interface UnlistedGroup {
  /** The folder holding the files (forward slashes; "C:" for a drive root, "/" for the POSIX root). */
  folder: string;
  files: number;
}

export interface Reconciliation {
  /** Indexed files inside no listed folder (forward slashes). */
  unlisted: string[];
  /** The folders of `unlisted`, none inside another, most files first. */
  groups: UnlistedGroup[];
}

/** `note://…`, `calendar://…`, `https://…`: indexed records that are not files in a folder. */
const SCHEME = /^[a-z][a-z0-9+.-]*:\/\//i;

/** Forward slashes, no trailing slash ("C:\\" → "C:", "/" → ""). */
function normalise(path: string): string {
  return path.replace(/\\/g, '/').replace(/\/+$/, '');
}

/** Whether `path` (normalised, lowercase) is `folder` or inside it. */
function within(path: string, folder: string): boolean {
  return path === folder || path.startsWith(`${folder}/`);
}

function parent(path: string): string {
  const slash = path.lastIndexOf('/');
  return slash < 0 ? '' : path.slice(0, slash);
}

/** The folder keys (normalised, lowercase) of listed folders; a blank path covers nothing. */
function folderKeys(folders: readonly { path: string }[]): string[] {
  // "/" normalises to "" and covers the POSIX root.
  return folders.filter(f => f.path.trim()).map(f => normalise(f.path).toLowerCase());
}

/**
 * The indexed files that are files in a folder (no scheme records), each once:
 * normalised, lowercase key to the normalised path, in index order.
 */
function indexedFileKeys(indexedFiles: readonly string[]): Map<string, string> {
  const files = new Map<string, string>();
  for (const raw of indexedFiles) {
    if (!raw || SCHEME.test(raw)) continue;
    const path = normalise(raw);
    if (!path) continue;
    const key = path.toLowerCase();
    if (!files.has(key)) files.set(key, path);
  }
  return files;
}

/**
 * The indexed files `list_indexed_sources` lists, each as it is shown: its
 * `display_path` (the file system's spelling, "C:\Research\papers\SPANN.pdf")
 * when that names the same file as its `source` (the identity, lowercased
 * on Windows), else its `source`. Entries without a string source are dropped.
 */
export function indexedFilePaths(docs: unknown): string[] {
  if (!Array.isArray(docs)) return [];
  const paths: string[] = [];
  for (const doc of docs) {
    if (typeof doc !== 'object' || doc === null) continue;
    const { source, display_path: display } = doc as { source?: unknown; display_path?: unknown };
    if (typeof source !== 'string') continue;
    if (typeof display === 'string' && display.trim() !== '' && normalise(display).toLowerCase() === normalise(source).toLowerCase()) {
      paths.push(display);
    } else {
      paths.push(source);
    }
  }
  return paths;
}

export function reconcileSources(
  folders: readonly { path: string }[],
  indexedFiles: readonly string[],
): Reconciliation {
  const covering = folderKeys(folders);
  const unlisted: string[] = [];
  for (const [key, path] of indexedFileKeys(indexedFiles)) {
    if (covering.some(folder => within(key, folder))) continue;
    unlisted.push(path);
  }

  // Each file's folder, then folders inside another merged into the outer one.
  const byFolder = new Map<string, UnlistedGroup>();
  for (const path of unlisted) {
    const folder = parent(path);
    const key = folder.toLowerCase();
    const group = byFolder.get(key);
    if (group) group.files += 1;
    else byFolder.set(key, { folder, files: 1 });
  }
  const keys = [...byFolder.keys()].sort((a, b) => a.length - b.length);
  const outer: string[] = [];
  for (const key of keys) {
    const host = outer.find(o => within(key, o));
    if (host === undefined) {
      outer.push(key);
      continue;
    }
    const hostGroup = byFolder.get(host);
    const group = byFolder.get(key);
    if (hostGroup && group) hostGroup.files += group.files;
    byFolder.delete(key);
  }
  const groups = [...byFolder.values()]
    .map(g => ({ folder: g.folder || '/', files: g.files }))
    .sort((a, b) => b.files - a.files || a.folder.toLowerCase().localeCompare(b.folder.toLowerCase()));
  return { unlisted, groups };
}

/**
 * A drive or share root ("C:", "//server/share", "/"): adding it as a folder
 * would index the whole disk, so the Library never offers to.
 */
export function isDriveRoot(folder: string): boolean {
  const path = normalise(folder);
  return path === '' || /^[a-z]:$/i.test(path) || /^\/\/[^/]+(\/[^/]+)?$/.test(path);
}

/**
 * The group folders "Add these folders" offers: never a drive root, the
 * home folder or a folder holding it (`broad`: home), whose whole tree
 * would be indexed, and never a folder holding one already listed (indexing
 * it would move that folder's files into the new source).
 */
export function addableFolders(
  groups: readonly UnlistedGroup[],
  folders: readonly { path: string }[],
  broad: readonly string[] = [],
): string[] {
  const keyOf = (path: string) => normalise(path).toLowerCase();
  const listed = folders.filter(f => f.path.trim()).map(f => keyOf(f.path));
  const wide = broad.filter(p => p.trim()).map(keyOf);
  return groups
    .map(g => g.folder)
    .filter(folder => {
      const key = keyOf(folder);
      return !isDriveRoot(folder) && !wide.some(path => within(path, key)) && !listed.some(path => within(path, key));
    });
}

/** Why `addableFolders` leaves a group folder out, as the Library and its notices say it. */
export const NOT_ADDED_REASON =
  'Their folders are not added for you: each is a whole drive, your home folder or a folder holding it, or holds a folder already here. Add the folders you want with Add folder.';

/** A group folder (forward slashes) in the form the folder indexer is given: backslashes on Windows. */
export function nativePath(folder: string, windows: boolean): string {
  return windows ? folder.replace(/\//g, '\\') : folder;
}

/** What answers search, as the sources chip and the Ask empty state read it. */
export interface SearchedSources {
  /** Folders listed in the Library. */
  folders: number;
  /** Folders included in Ask. */
  included: number;
  /** Answers search only the included folders: some are included, not all. */
  limited: boolean;
  /** Indexed files answers search; null while the index has not been listed. */
  files: number | null;
  /** Of `files`, those inside no listed folder (0 when limited: answers skip them). */
  unlisted: number;
  /**
   * Whether there is anything to search: a listed folder (even one whose
   * indexing failed, which the Library shows) or an indexed file.
   */
  hasSources: boolean;
}

/**
 * What answers search, counted in files from the index list: every indexed
 * file, or when only some folders are included, the files inside those.
 * `indexedFiles` is null until `list_indexed_sources` has been read.
 */
export function searchedSources(
  folders: readonly { path: string; selected: boolean }[],
  indexedFiles: readonly string[] | null,
): SearchedSources {
  const included = folders.filter(f => f.selected).length;
  const limited = included > 0 && included < folders.length;
  const covering = folderKeys(folders);
  const includedFolders = folderKeys(folders.filter(f => f.selected));
  const indexed = indexedFileKeys(indexedFiles ?? []);
  let files = 0;
  let unlisted = 0;
  for (const key of indexed.keys()) {
    if (limited) {
      if (includedFolders.some(folder => within(key, folder))) files += 1;
      continue;
    }
    files += 1;
    if (!covering.some(folder => within(key, folder))) unlisted += 1;
  }
  return {
    folders: folders.length,
    included,
    limited,
    files: indexedFiles === null ? null : files,
    unlisted,
    hasSources: folders.length > 0 || indexed.size > 0,
  };
}

/**
 * The sources chip of the top bar and of the composer (outside a
 * workspace), so the two always agree: what answers search, as
 * `searchedSources` counts it.
 */
export function librarySourcesChip(searched: SearchedSources): { label: string; title: string } {
  if (!searched.hasSources) return { label: 'No sources yet', title: 'Add a folder in Library' };
  if (searched.limited) {
    const files = searched.files === null ? '' : ` (${countOf(searched.files, 'indexed file')})`;
    const count = searched.files === null ? '' : ` · ${countOf(searched.files, 'file')}`;
    return {
      label: `${searched.included.toLocaleString()} of ${countOf(searched.folders, 'folder')}${count}`,
      title: `Answers search only the ${countOf(searched.included, 'folder')} included in Ask${files}. Manage sources in Library.`,
    };
  }
  if (searched.files === null) {
    return { label: 'All sources', title: 'Answers search everything you have indexed. Manage sources in Library.' };
  }
  const unlisted = searched.unlisted > 0
    ? ` ${countOf(searched.unlisted, 'file')} ${searched.unlisted === 1 ? 'is' : 'are'} in no folder listed in Library.`
    : '';
  return {
    label: `All sources · ${countOf(searched.files, 'file')}`,
    title: `Answers search the ${countOf(searched.files, 'file')} you have indexed.${unlisted} Manage sources in Library.`,
  };
}
