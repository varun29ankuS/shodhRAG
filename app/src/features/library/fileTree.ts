/**
 * Library file browser model: a folder tree built from what the index knows
 * about a source (indexed files from `get_source_files`, failures from the
 * last indexing run), plus sorting, filtering and file-type helpers.
 *
 * Pure module (no runtime imports) so it is unit-tested directly with Node
 * (`app/tests/fileTree.test.ts`).
 */

/** Row of `get_source_files` (snake_case `FileInfo` in rag_commands.rs). */
export interface IndexedFileRow {
  name: string;
  file_path: string;
  file_type: string;
  status: string;
}

/** One failed file from an indexing run (`FileFailure`). */
export interface FileFailure {
  file: string;
  reason: string;
}

export type FileStatus = 'indexed' | 'failed';

export interface FileNode {
  kind: 'file';
  name: string;
  /** Absolute path as the backend reported it. */
  path: string;
  extension: string;
  status: FileStatus;
  /** Why indexing failed, for failed files. */
  reason?: string;
  /** Folder names from the source root down to this file's folder. */
  dir: string[];
}

export interface DirNode {
  kind: 'dir';
  name: string;
  /** Folder names from the source root down to and including this folder. */
  dir: string[];
  dirs: Map<string, DirNode>;
  files: FileNode[];
}

export type TreeEntry = DirNode | FileNode;

/** Windows paths compare case-insensitively (the index lowercases them). */
function isWindowsPath(path: string): boolean {
  return /^[a-zA-Z]:[\\/]/.test(path) || path.startsWith('\\\\');
}

/** Comparable form of a path: forward slashes, no trailing slash, lowercased on Windows. */
export function pathKey(path: string): string {
  const slashed = path.replace(/\\/g, '/').replace(/\/+$/, '');
  return isWindowsPath(path) ? slashed.toLowerCase() : slashed;
}

/** Last path segment. */
export function baseName(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/').filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** Lowercase extension without the dot, or '' when there is none. */
export function extensionOf(name: string): string {
  const base = baseName(name);
  const dot = base.lastIndexOf('.');
  return dot > 0 ? base.slice(dot + 1).toLowerCase() : '';
}

/**
 * Folder segments of `filePath` below `rootPath`, or null when the file is
 * not under the root. Segment case comes from `filePath`.
 */
export function relativeDir(rootPath: string, filePath: string): string[] | null {
  const root = pathKey(rootPath);
  const file = pathKey(filePath);
  if (!file.startsWith(`${root}/`)) return null;
  // Lowercasing keeps ASCII lengths, so the key's offsets index the original.
  const original = filePath.replace(/\\/g, '/').replace(/\/+$/, '');
  const rest = original.length === file.length ? original.slice(root.length + 1) : file.slice(root.length + 1);
  const parts = rest.split('/').filter(Boolean);
  return parts.slice(0, -1);
}

function newDir(name: string, dir: string[]): DirNode {
  return { kind: 'dir', name, dir, dirs: new Map(), files: [] };
}

function ensureDir(root: DirNode, segments: string[]): DirNode {
  let node = root;
  for (let i = 0; i < segments.length; i++) {
    const key = segments[i].toLowerCase();
    let next = node.dirs.get(key);
    if (!next) {
      next = newDir(segments[i], segments.slice(0, i + 1));
      node.dirs.set(key, next);
    }
    node = next;
  }
  return node;
}

/**
 * Build the folder tree of a source. Indexed files come first; a failure for
 * a file that is also indexed (e.g. it failed once, then succeeded) is
 * dropped. Files outside the source root are listed at the top level.
 */
export function buildFileTree(rootPath: string, rows: readonly IndexedFileRow[], failures: readonly FileFailure[]): DirNode {
  const root = newDir(baseName(rootPath), []);
  const seen = new Set<string>();

  const add = (path: string, name: string, status: FileStatus, reason?: string) => {
    const key = pathKey(path);
    if (seen.has(key)) return;
    seen.add(key);
    const dir = relativeDir(rootPath, path) ?? [];
    const parent = ensureDir(root, dir);
    parent.files.push({ kind: 'file', name, path, extension: extensionOf(name || path), status, reason, dir: parent.dir });
  };

  for (const row of rows) {
    if (!row.file_path) continue;
    // `name` may be a document title rather than the file name; prefer the path's.
    add(row.file_path, baseName(row.file_path), 'indexed');
  }
  for (const failure of failures) {
    if (!failure.file) continue;
    add(failure.file, baseName(failure.file), 'failed', failure.reason);
  }
  return root;
}

/** The folder at `segments` (case-insensitive), or null if it does not exist. */
export function findDir(root: DirNode, segments: readonly string[]): DirNode | null {
  let node: DirNode | undefined = root;
  for (const segment of segments) {
    node = node.dirs.get(segment.toLowerCase());
    if (!node) return null;
  }
  return node;
}

export interface TreeCounts {
  files: number;
  indexed: number;
  failed: number;
}

/** File counts of a folder and everything below it. */
export function countTree(node: DirNode): TreeCounts {
  const counts: TreeCounts = { files: 0, indexed: 0, failed: 0 };
  const walk = (dir: DirNode) => {
    for (const f of dir.files) {
      counts.files += 1;
      if (f.status === 'indexed') counts.indexed += 1;
      else counts.failed += 1;
    }
    for (const child of dir.dirs.values()) walk(child);
  };
  walk(node);
  return counts;
}

export type SortKey = 'name' | 'type' | 'status';

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });

/**
 * Entries of one folder: subfolders first (always by name), then files by
 * `key`, ties broken by name.
 */
export function folderEntries(dir: DirNode, key: SortKey, descending = false): TreeEntry[] {
  const dirs = [...dir.dirs.values()].sort((a, b) => collator.compare(a.name, b.name));
  if (descending && key === 'name') dirs.reverse();
  const sign = descending ? -1 : 1;
  const files = [...dir.files].sort((a, b) => {
    let primary = 0;
    if (key === 'type') primary = collator.compare(a.extension, b.extension);
    else if (key === 'status') primary = a.status === b.status ? 0 : a.status === 'failed' ? -1 : 1;
    if (primary !== 0) return primary * sign;
    return collator.compare(a.name, b.name) * (key === 'name' ? sign : 1);
  });
  return [...dirs, ...files];
}

/**
 * Files below `dir` whose name contains every word of `query`
 * (case-insensitive), sorted by `key`. Used when the browser is filtered.
 */
export function searchFiles(dir: DirNode, query: string, key: SortKey, descending = false): FileNode[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [];
  const out: FileNode[] = [];
  const walk = (node: DirNode) => {
    for (const f of node.files) {
      const name = f.name.toLowerCase();
      if (words.every(w => name.includes(w))) out.push(f);
    }
    for (const child of node.dirs.values()) walk(child);
  };
  walk(dir);
  const holder: DirNode = { kind: 'dir', name: '', dir: [], dirs: new Map(), files: out };
  return folderEntries(holder, key, descending) as FileNode[];
}

/** Short uppercase badge for a file type, e.g. "PDF", "XLSX". */
export function typeBadge(extension: string): string {
  if (!extension) return 'FILE';
  return extension.toUpperCase().slice(0, 4);
}

/** Broad family of a file type, used to colour its badge. */
export type TypeFamily = 'pdf' | 'doc' | 'sheet' | 'slides' | 'text' | 'code' | 'image' | 'other';

const FAMILIES: Record<string, TypeFamily> = {
  pdf: 'pdf',
  doc: 'doc', docx: 'doc', odt: 'doc', rtf: 'doc',
  xls: 'sheet', xlsx: 'sheet', xlsm: 'sheet', xlsb: 'sheet', ods: 'sheet', csv: 'sheet', tsv: 'sheet',
  ppt: 'slides', pptx: 'slides', odp: 'slides',
  txt: 'text', md: 'text', markdown: 'text', rst: 'text', log: 'text', html: 'text', htm: 'text',
  png: 'image', jpg: 'image', jpeg: 'image', gif: 'image', bmp: 'image', webp: 'image', tif: 'image', tiff: 'image',
};

const CODE = new Set(['rs', 'js', 'ts', 'tsx', 'jsx', 'py', 'java', 'c', 'cpp', 'h', 'hpp', 'cs', 'go', 'rb', 'php', 'swift', 'kt', 'json', 'yaml', 'yml', 'toml', 'xml', 'sql', 'sh']);

export function typeFamily(extension: string): TypeFamily {
  return FAMILIES[extension] ?? (CODE.has(extension) ? 'code' : 'other');
}
