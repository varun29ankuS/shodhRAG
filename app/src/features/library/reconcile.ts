/**
 * The Library's folders against what the index holds: the indexed files that
 * no listed folder covers, and the folders they are in. The Library shows
 * them so it never says "nothing added" while answers cite indexed files.
 *
 * Paths compare as Windows paths do: forward slashes, case-insensitively.
 * Pure module (no runtime imports) so it is unit-tested directly with Node
 * (`app/tests/libraryReconcile.test.ts`).
 */

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
 * The group folders "Add these folders" adds: not a drive root, and not a
 * folder holding one already listed (indexing it would move that folder's
 * files into the new source).
 */
export function addableFolders(groups: readonly UnlistedGroup[], folders: readonly { path: string }[]): string[] {
  const listed = folders.filter(f => f.path.trim()).map(f => normalise(f.path).toLowerCase());
  return groups
    .map(g => g.folder)
    .filter(folder => !isDriveRoot(folder) && !listed.some(path => within(path, normalise(folder).toLowerCase())));
}

/**
 * The unlisted files that can be removed from the index one at a time. The
 * removal matches by path prefix, so a file whose path starts another indexed
 * file that stays (e.g. `a.pdf` and a listed `a.pdf.md`) is left in place.
 */
export function safeRemovals(unlisted: readonly string[], indexedFiles: readonly string[]): string[] {
  const leaving = new Set(unlisted.map(p => normalise(p).toLowerCase()));
  const staying = indexedFiles
    .filter(p => p && !SCHEME.test(p))
    .map(p => normalise(p).toLowerCase())
    .filter(p => !leaving.has(p));
  return unlisted.filter(p => {
    const key = normalise(p).toLowerCase();
    return !staying.some(other => other.startsWith(key));
  });
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
