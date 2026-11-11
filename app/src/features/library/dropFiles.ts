/**
 * What happens to each path dropped on the window: images have their text read
 * (OCR) into the chat; folders and the files the indexer reads (Word and
 * PowerPoint in the old .doc and .ppt formats too) are added to the Library;
 * other files are named as not indexed. The drop overlay promises "PDF, Word,
 * Excel, PowerPoint, text, Markdown or images". No imports: unit-tested under Node.
 */

/**
 * Every extension the indexer reads: `shodh_rag::indexing::SUPPORTED_FILE_EXTENSIONS`
 * (kept equal by `tests/dropFiles.test.ts`).
 */
export const INDEXABLE_EXTENSIONS: readonly string[] = [
  'txt', 'md', 'pdf', 'html', 'htm', 'json', 'docx', 'pptx', 'rst', 'tex',
  'doc', 'ppt', 'odt', 'odp', 'rtf', 'epub', 'eml', 'ipynb', 'log',
  'csv', 'tsv', 'xlsx', 'xls', 'xlsm', 'xlsb', 'ods',
  'rs', 'py', 'js', 'ts', 'jsx', 'tsx', 'java', 'cpp', 'c', 'h', 'hpp', 'cs', 'go', 'rb', 'php',
  'swift', 'kt', 'scala', 'r', 'sh', 'bash', 'zsh', 'ps1', 'bat', 'cmd',
  'css', 'scss', 'sass', 'less', 'vue', 'svelte',
  'toml', 'yaml', 'yml', 'ini', 'conf', 'config', 'xml', 'sql', 'graphql', 'proto',
  'png', 'jpg', 'jpeg', 'bmp', 'tiff', 'tif', 'gif', 'webp',
];

/** Images whose text is read when dropped (`process_image_from_file`). */
export const DROP_IMAGE_EXTENSIONS: readonly string[] = ['png', 'jpg', 'jpeg', 'gif', 'bmp', 'webp'];

/**
 * Documents the indexer reads (a subset of `shodh_rag::indexing::SUPPORTED_FILE_EXTENSIONS`):
 * the formats a chat upload takes.
 */
export const DROP_DOCUMENT_EXTENSIONS: readonly string[] = [
  'pdf', 'docx', 'pptx', 'doc', 'ppt', 'odt', 'odp', 'rtf', 'epub', 'eml', 'xlsx', 'xls', 'csv', 'txt', 'md', 'html', 'htm', 'json',
];

/** The last part of a Windows or POSIX path. */
function baseName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** The lowercase extension of a path or file name, without the dot ('' when none). */
export function extensionOf(path: string): string {
  const name = baseName(path);
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

export function isDroppedImage(path: string): boolean {
  return DROP_IMAGE_EXTENSIONS.includes(extensionOf(path));
}

export function isIndexableDocument(path: string): boolean {
  return DROP_DOCUMENT_EXTENSIONS.includes(extensionOf(path));
}

/** Whether the indexer reads a file at `path` (a folder is decided by the file system, not here). */
export function isIndexableFile(path: string): boolean {
  return INDEXABLE_EXTENSIONS.includes(extensionOf(path));
}

/** What to tell the person about dropped files of a type the indexer does not read. */
export function unsupportedFilesNotice(paths: readonly string[]): string {
  const names = paths.map(baseName).join(', ');
  return `${names} ${paths.length === 1 ? 'is' : 'are'} not a type Shodh can index (PDF, Word, Excel, PowerPoint, OpenDocument, RTF, EPUB, email, text, Markdown, code or images).`;
}

export interface DroppedPaths {
  images: string[];
  /**
   * Documents, folders and other files, in drop order: folders and indexable files
   * are added to the Library (`isIndexableFile`, once the file system says which is a folder).
   */
  library: string[];
}

export function sortDroppedPaths(paths: readonly string[]): DroppedPaths {
  const sorted: DroppedPaths = { images: [], library: [] };
  for (const path of paths) {
    if (isDroppedImage(path)) sorted.images.push(path);
    else sorted.library.push(path);
  }
  return sorted;
}
