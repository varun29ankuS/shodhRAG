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

export function reconcileSources(
  folders: readonly { path: string }[],
  indexedFiles: readonly string[],
): Reconciliation {
  // A blank path covers nothing; "/" normalises to "" and covers the POSIX root.
  const covering = folders.filter(f => f.path.trim()).map(f => normalise(f.path).toLowerCase());
  const seen = new Set<string>();
  const unlisted: string[] = [];
  for (const raw of indexedFiles) {
    if (!raw || SCHEME.test(raw)) continue;
    const path = normalise(raw);
    if (!path) continue;
    const key = path.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
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

/**
 * How many sources answers search: the included folders when only some are
 * included (answers then search just those), else every folder and every
 * indexed file outside them.
 */
export function searchedSourceCount(folders: readonly { selected: boolean }[], unlistedFiles: number): number {
  const included = folders.filter(f => f.selected).length;
  const limited = included > 0 && included < folders.length;
  return limited ? included : folders.length + unlistedFiles;
}

/**
 * The sources chip of the top bar and of the composer (outside a
 * workspace), so the two always agree: what answers search, as
 * `searchedSourceCount` counts it.
 */
export function librarySourcesChip(
  folders: readonly { selected: boolean }[],
  unlistedFiles: number,
): { label: string; title: string } {
  const count = searchedSourceCount(folders, unlistedFiles);
  if (count === 0) return { label: 'No sources yet', title: 'Add a folder in Library' };
  const included = folders.filter(f => f.selected).length;
  if (included > 0 && included < folders.length) {
    return {
      label: `${included.toLocaleString()} of ${countOf(folders.length, 'folder')}`,
      title: `Answers search only the ${countOf(included, 'folder')} included in Ask. Manage sources in Library.`,
    };
  }
  return {
    label: `All sources · ${count.toLocaleString()}`,
    title: unlistedFiles > 0
      ? `Answers search everything you have indexed: ${folders.length > 0 ? `${countOf(folders.length, 'folder')} and ` : ''}${countOf(unlistedFiles, 'indexed file')} in no folder. Manage sources in Library.`
      : 'Answers search everything you have indexed. Manage sources in Library.',
  };
}
